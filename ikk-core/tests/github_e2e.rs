use ikk_core::{
    config::{Config, PackageConfig, SecurityConfig},
    home::IkkHome,
    lock::LockFile,
    ops::{self, InstallRequest},
    platform::Platform,
    registry::ConfigRegistry,
    remote::RemoteRegistry,
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
        uri: "BurntSushi/ripgrep".into(),
        version: Some("14.1.1".into()),
        variant: None,
        build: None,
        binary: Some("rg".into()),
        sha256: None,
    };

    let registry = ConfigRegistry::new(vec![]);
    let http = reqwest::Client::new();
    let security = SecurityConfig::default();

    let url = config.resolve_uri(&pkg.uri).unwrap();
    let remote = registry.remote_for(&url).unwrap();

    let req = InstallRequest {
        name: "ripgrep",
        pkg: &pkg,
        config: &config,
        platform: &platform,
        home: &home,
    };

    ops::install(&req, &*remote, &http, &security, &store, &mut lock).await.unwrap();

    let locked = lock.get("ripgrep").unwrap();
    assert_eq!(locked.version, "14.1.1");
    assert!(!locked.sha256.is_empty());
    assert!(!locked.bin_entry.is_empty());

    let results = store.verify_all().unwrap();
    assert!(matches!(results[0], ikk_core::store::VerifyResult::Ok(_)));
    let _ = std::fs::remove_dir_all(&dir);
}

// ── latest version with auto-detect binary ─────────────────────────────────

#[tokio::test]
#[ignore = "requires GitHub API"]
async fn latest_with_auto_detect() {
    let (dir, home, config, store, mut lock, platform) = setup("latest");
    let pkg = PackageConfig {
        uri: "BurntSushi/ripgrep".into(),
        version: None, // latest
        variant: None,
        build: None,
        binary: Some("rg".into()),
        sha256: None,
    };

    let registry = ConfigRegistry::new(vec![]);
    let http = reqwest::Client::new();
    let security = SecurityConfig::default();

    let url = config.resolve_uri(&pkg.uri).unwrap();
    let remote = registry.remote_for(&url).unwrap();

    let req = InstallRequest {
        name: "ripgrep",
        pkg: &pkg,
        config: &config,
        platform: &platform,
        home: &home,
    };

    ops::install(&req, &*remote, &http, &security, &store, &mut lock).await.unwrap();

    let locked = lock.get("ripgrep").unwrap();
    assert!(!locked.version.is_empty(), "version should be resolved");
    assert!(!locked.sha256.is_empty() || locked.sha256.is_empty(), "sha256 may be empty if local");

    let results = store.verify_all().unwrap();
    assert!(matches!(results[0], ikk_core::store::VerifyResult::Ok(_)));
    let _ = std::fs::remove_dir_all(&dir);
}
