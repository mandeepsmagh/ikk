use ikk_core::{
    config::{Config, PackageConfig, SecurityConfig},
    home::IkkHome,
    lock::LockFile,
    ops::{self, InstallRequest},
    platform::Platform,
    registry::ConfigRegistry,
    store::Store,
};
use std::path::PathBuf;

fn setup(name: &str) -> (PathBuf, IkkHome, Config, Store, LockFile, Platform) {
    let dir = std::env::temp_dir().join(format!("ikk_ci_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    let home = IkkHome::new(dir.join(".ikk"));
    home.init_dirs().unwrap();

    let mut config = Config::default();
    config.defaults.remote = Some("github.com".into());
    config.save(&home.config_file()).unwrap();

    let store = Store::open(home.store_dir()).unwrap();
    let lock = LockFile::load(&home.lock_file()).unwrap();
    let platform = Platform::current();

    (dir, home, config, store, lock, platform)
}

// ── pinned version with explicit binary ──────────────────────────────────────

#[tokio::test]
#[ignore = "requires GitHub API"]
async fn pinned_with_binary() {
    let (dir, home, config, store, mut lock, platform) = setup("pinned");
    let pkg = PackageConfig {
        source: "BurntSushi/ripgrep".into(),
        version: "14.1.1".into(),
        binary: Some("rg".into()),
        build: None,
        min_release_age_days: None,
    };

    let registry = ConfigRegistry::new(vec![]);
    let http = reqwest::Client::new();
    let security = SecurityConfig::default();
    let source = ops::make_source(&pkg, &config, &registry, &http, &security).unwrap();
    let req = InstallRequest {
        name: "ripgrep",
        pkg: &pkg,
        config: &config,
        platform: &platform,
        home: &home,
    };

    ops::install(&req, &*source, &store, &mut lock).await.unwrap();

    let locked = lock.get("ripgrep").unwrap();
    assert_eq!(locked.version, "14.1.1");
    assert!(!locked.binary_sha256.is_empty());
    assert!(!locked.archive_sha256.is_empty());

    let results = store.verify_all().unwrap();
    assert!(matches!(results[0], ikk_core::store::VerifyResult::Ok(_)));
    let _ = std::fs::remove_dir_all(&dir);
}

// ── latest version with explicit binary ─────────────────────────────────────

#[tokio::test]
#[ignore = "requires GitHub API"]
async fn latest_with_binary() {
    let (dir, home, config, store, mut lock, platform) = setup("latest");
    let pkg = PackageConfig {
        source: "BurntSushi/ripgrep".into(),
        version: "latest".into(),
        binary: Some("rg".into()),
        build: None,
        min_release_age_days: None,
    };

    let registry = ConfigRegistry::new(vec![]);
    let http = reqwest::Client::new();
    let security = SecurityConfig::default();
    let source = ops::make_source(&pkg, &config, &registry, &http, &security).unwrap();
    let req = InstallRequest {
        name: "ripgrep",
        pkg: &pkg,
        config: &config,
        platform: &platform,
        home: &home,
    };

    ops::install(&req, &*source, &store, &mut lock).await.unwrap();

    let locked = lock.get("ripgrep").unwrap();
    assert!(!locked.version.is_empty(), "version should be resolved");
    assert!(!locked.binary_sha256.is_empty());

    let results = store.verify_all().unwrap();
    assert!(matches!(results[0], ikk_core::store::VerifyResult::Ok(_)));
    let _ = std::fs::remove_dir_all(&dir);
}
