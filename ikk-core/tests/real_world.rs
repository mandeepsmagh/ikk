#[cfg(test)]
mod real_world_tests {
    use ikk_core::extract::{count_binaries, extract, extract_dir};

    #[test]
    fn neovim_macos_binary_detection() {
        let tarball = "nvim-macos-arm64.tar.gz";
        if !std::path::Path::new(tarball).exists() {
            eprintln!("skipping — {tarball} not found");
            return;
        }
        let bytes = std::fs::read(tarball).expect("nvim tarball not found in cwd");
        let dir = std::env::temp_dir().join("ikk_test_nvim");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Extract full directory
        let extracted = extract_dir(&bytes, "nvim-macos-arm64.tar.gz", &dir).unwrap();
        assert!(extracted.exists());

        // Count binaries (should be 1 — nvim, not the .so parser libs)
        let count = count_binaries(&extracted).unwrap();
        assert_eq!(
            count, 1,
            "expected 1 binary (nvim), got {count} — .so files should be excluded"
        );

        // The single binary extraction should work
        let binary = extract(&bytes, "nvim-macos-arm64.tar.gz", "neovim", &dir).unwrap();
        assert_eq!(binary.file_name().unwrap(), "nvim");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
