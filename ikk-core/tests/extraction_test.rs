use std::path::PathBuf;
use ikk_core::extract::extract;

#[test]
fn test_tar_gz_extraction() {
    // Get the absolute path to the ikk-core directory
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    
    // The archive is now located inside the crate's own test assets directory.
    // This makes the test self-contained and portable.
    let archive_path = manifest_dir.join("tests/assets/nvim-linux-x86_64.tar.gz");
    
    println!("Checking archive at: {:?}", archive_path);

    assert!(archive_path.exists(), "Test archive not found at: {:?}", archive_path);

    let bytes = std::fs::read(&archive_path).expect("Failed to read test archive");
    
    // Setup temporary stage directory
    let stage_dir = std::env::temp_dir().join("ikk_test_extraction");
    if stage_dir.exists() {
        let _ = std::fs::remove_dir_all(&stage_dir);
    }
    std::fs::create_dir_all(&stage_dir).unwrap();

    // Run extraction
    let result = extract(&bytes, "nvim-linux-x86_64.tar.gz", "nvim", &stage_dir);

    // Assertions
    assert!(result.is_ok(), "Extraction failed: {:?}", result.err());
    let extracted_path = result.unwrap();
    assert!(extracted_path.exists(), "Extracted binary does not exist");
    assert!(extracted_path.is_file(), "Extracted path is not a file");
    
    println!("Successfully extracted to: {:?}", extracted_path);

    // Cleanup
    let _ = std::fs::remove_dir_all(stage_dir);
}
