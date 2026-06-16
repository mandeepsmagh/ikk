/// Integration test: full install pipeline against real GitHub.
/// Downloads ripgrep, extracts, stores, verifies. Runs only in CI.
#[tokio::test]
#[ignore = "requires GitHub API access — run with: cargo test -- --include-ignored"]
async fn install_ripgrep_from_github() {
    // ── setup ──────────────────────────────────────────────────────────
    let dir = std::env::temp_dir().join("ikk_ci_test");
    let _ = std::fs::remove_dir_all(&dir);
    let home = ikk_core::IkkHome::new(dir.join(".ikk"));
    home.init_dirs().unwrap();

    let mut config = ikk_core::config::Config::default();
    config.defaults.remote = Some("github.com".into());
    config.save(&home.config_file()).unwrap();

    let pkg = ikk_core::config::PackageConfig {
        source: "BurntSushi/ripgrep".into(),
        version: "14.1.1".into(),
        binary: Some("rg".into()),
        build: None,
        min_release_age_days: None,
    };

    let registry = ikk_core::registry::ConfigRegistry::new(vec![]);
    let http = reqwest::Client::new();
    let security = ikk_core::config::SecurityConfig::default();
    let store = ikk_core::store::Store::open(home.store_dir()).unwrap();
    let mut lock = ikk_core::lock::LockFile::load(&home.lock_file()).unwrap();
    let platform = ikk_core::platform::Platform::current();

    // ── install ────────────────────────────────────────────────────────
    let source = ikk_core::ops::make_source(&pkg, &config, &registry, &http, &security).unwrap();

    let req = ikk_core::ops::InstallRequest {
        name: "ripgrep",
        pkg: &pkg,
        config: &config,
        platform: &platform,
        home: &home,
    };

    ikk_core::ops::install(&req, &*source, &store, &mut lock).await.unwrap();

    // ── verify ─────────────────────────────────────────────────────────
    let locked = lock.get("ripgrep").expect("package not in lock");
    assert_eq!(locked.version, "14.1.1");
    assert!(!locked.binary_sha256.is_empty(), "missing binary hash");
    assert!(!locked.archive_sha256.is_empty(), "missing archive hash");

    let results = store.verify_all().unwrap();
    assert_eq!(results.len(), 1, "expected 1 package in store");
    assert!(matches!(results[0], ikk_core::store::VerifyResult::Ok(_)));

    let _ = std::fs::remove_dir_all(&dir);
}
