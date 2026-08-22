use crate::error::Result;
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};

/// Truncate a label to fit within a display width, appending `…` if cut.
fn truncate_label(label: &str, max: usize) -> String {
    if label.len() <= max {
        return label.to_string();
    }

    // Cut at the last char boundary within `max - 1` (room for the ellipsis),
    // so a multi-byte UTF-8 label can never panic on a mid-codepoint slice.
    let cut = label
        .char_indices()
        .map(|(idx, ch)| idx + ch.len_utf8())
        .take_while(|end| *end <= max.saturating_sub(1))
        .last()
        .unwrap_or(0);

    format!("{}…", &label[..cut])
}

/// Create a progress bar or spinner for a download.
/// Uses a spinner when content length is unknown, a progress bar otherwise.
pub(crate) fn download_bar(total: u64, label: &str) -> ProgressBar {
    let display = truncate_label(label, 32);

    if total == 0 {
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::default_spinner()
                .template("  {msg:34} {spinner} {bytes:>10} {bytes_per_sec:>10}")
                .unwrap(),
        );
        bar.set_message(display);
        bar
    } else {
        let bar = ProgressBar::new(total);
        bar.set_style(
            ProgressStyle::default_bar()
                .template(
                    "  {msg:34} [{bar:30.cyan/blue}] {bytes:>7}/{total_bytes:7} {bytes_per_sec:>10} {eta:>4}",
                )
                .unwrap()
                .progress_chars("=> "),
        );
        bar.set_message(display);
        bar
    }
}

/// Stream download with progress display.
///
/// The `reqwest::Client` should be configured with a timeout — a stalled
/// connection will otherwise hang indefinitely. `bearer` is attached as an
/// `Authorization` header when present (private-repo release assets).
pub async fn download_bytes(
    http: &reqwest::Client,
    url: &str,
    label: &str,
    bearer: Option<&str>,
) -> Result<Vec<u8>> {
    let mut req = http.get(url);
    if let Some(token) = bearer {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await?.error_for_status()?;
    let total = resp.content_length().unwrap_or(0);
    let bar = download_bar(total, label);

    let mut buf = Vec::with_capacity(usize::try_from(total).unwrap_or(0));
    let mut stream = resp.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buf.extend_from_slice(&chunk);
        bar.inc(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
    }

    bar.finish_with_message(format!("✓ {label}"));
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_label_handles_utf8_boundary() {
        assert_eq!(truncate_label("héllo", 3), "h…");
        assert_eq!(truncate_label("hello", 6), "hello");
        assert_eq!(truncate_label("hello!", 5), "hell…");
    }
}
