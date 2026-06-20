use crate::error::Result;
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};

/// Create a progress bar styled for file downloads.
#[must_use]
pub fn download_bar(total: u64, label: &str) -> ProgressBar {
    let bar = ProgressBar::new(total);
    bar.set_style(
        ProgressStyle::default_bar()
            .template(
                "  {msg:20} [{bar:30.cyan/blue}] {bytes:>7}/{total_bytes:7} {bytes_per_sec:>10} {eta:>4}",
            )
            .unwrap()
            .progress_chars("=> "),
    );
    bar.set_message(label.to_string());
    bar
}

/// Stream download with progress bar. Returns the downloaded bytes.
pub async fn download_bytes(http: &reqwest::Client, url: &str, label: &str) -> Result<Vec<u8>> {
    let resp = http.get(url).send().await?;
    let total = resp.content_length().unwrap_or(0);
    let bar = download_bar(total, label);

    let mut buf = Vec::with_capacity(usize::try_from(total).unwrap_or(0));
    let mut stream = resp.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buf.extend_from_slice(&chunk);
        bar.inc(chunk.len() as u64);
    }

    bar.finish_with_message(format!("✓ {label}"));
    Ok(buf)
}
