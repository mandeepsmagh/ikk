#[cfg(test)]
mod real_world_tests {
    use ikk_core::processor::extract_dir;

    #[test]
    fn neovim_macos_directory_extraction() {
        let tarball = "nvim-macos-arm64.tar.gz";
        if !std::path::Path::new(tarball).exists() {
            eprintln!("skipping — {tarball} not found");
            return;
        }
        let bytes = std::fs::read(tarball).expect("nvim tarball not found in cwd");
        let dir = std::env::temp_dir().join("ikk_test_nvim");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Extract the full package directory; unwrap_single_root descends into
        // the lone top-level dir, so the neovim package root contains `bin/nvim`.
        let extracted = extract_dir(&bytes, "nvim-macos-arm64.tar.gz", &dir).unwrap();
        assert!(extracted.exists());
        assert!(
            extracted.join("bin/nvim").exists(),
            "expected nvim binary at bin/nvim in the package root"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
