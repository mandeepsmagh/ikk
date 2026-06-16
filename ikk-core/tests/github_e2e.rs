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

fn setup() -> (PathBuf, IkkHome, Config, Store, LockFile, Platform) {
    let dir = std::env::temp_dir().join("ikk_ci_test");
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

fn make_req<'a>(
    name: &'a str,
    pkg: &'a PackageConfig,
    config: &'a Config,
    platform: &'a Platform,
    home: &'a IkkHome,
) -> InstallRequest<'a> {
    InstallRequest { name, pkg, config, platform, home }
}

// ── pinned version with explicit binary ──────────────────────────────────────

#[tokio::test]
#[ignore = "requires GitHub API"]
async fn pinned_with_binary() {
    let (dir, home, config, store, mut lock, platform) = setup();
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
    let req = make_req("ripgrep", &pkg, &config, &platform, &home);

    ops::install(&req, &*source, &store, &mut lock).await.unwrap();

    let locked = lock.get("ripgrep").unwrap();
    assert_eq!(locked.version, "14.1.1");
    assert!(!locked.binary_sha256.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

// ── latest version with explicit binary ─────────────────────────────────────

#[tokio::test]
#[ignore = "requires GitHub API"]
async fn latest_with_binary() {
    let (dir, home, config, store, mut lock, platform) = setup();
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
    let req = make_req("ripgrep", &pkg, &config, &platform, &home);

    ops::install(&req, &*source, &store, &mut lock).await.unwrap();

    let locked = lock.get("ripgrep").unwrap();
    assert!(!locked.version.is_empty(), "version should be resolved");
    assert!(!locked.binary_sha256.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

// ── auto-detect binary name (fd → fd) ───────────────────────────────────────

#[tokio::test]
#[ignore = "requires GitHub API"]
async fn auto_detect_binary() {
    let (dir, home, config, store, mut lock, platform) = setup();
    // fd's binary is also called "fd" — tests the no-flag path works
    let pkg = PackageConfig {
        source: "sharkdp/fd".into(),
        version: "10.2.0".into(),
        binary: None, // auto-detect
        build: None,
        min_release_age_days: None,
    };

    let registry = ConfigRegistry::new(vec![]);
    let http = reqwest::Client::new();
    let security = SecurityConfig::default();
    let source = ops::make_source(&pkg, &config, &registry, &http, &security).unwrap();
    let req = make_req("fd", &pkg, &config, &platform, &home);

    ops::install(&req, &*source, &store, &mut lock).await.unwrap();

    let locked = lock.get("fd").unwrap();
    assert_eq!(locked.version, "10.2.0");
    assert!(!locked.binary_sha256.is_empty());
    assert!(home.bin_dir().join("fd").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

// ── sync with two packages ──────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires GitHub API"]
async fn sync_two_packages() {
    let (dir, home, mut config, store, mut lock, platform) = setup();

    config.packages.insert(
        "ripgrep".into(),
        PackageConfig {
            source: "BurntSushi/ripgrep".into(),
            version: "14.1.1".into(),
            binary: Some("rg".into()),
            build: None,
            min_release_age_days: None,
        },
    );
    config.packages.insert(
        "fd".into(),
        PackageConfig {
            source: "sharkdp/fd".into(),
            version: "10.2.0".into(),
            binary: None,
            build: None,
            min_release_age_days: None,
        },
    );

    let registry = ConfigRegistry::new(vec![]);
    let http = reqwest::Client::new();
    let security = SecurityConfig::default();

    let report = ops::sync(
        &config,
        &security,
        &home,
        &registry,
        &store,
        &mut lock,
        &home.lock_file(),
        &http,
        &platform,
    )
    .await
    .unwrap();

    assert_eq!(report.installed.len(), 2);
    assert!(lock.get("ripgrep").is_some());
    assert!(lock.get("fd").is_some());

    let results = store.verify_all().unwrap();
    assert_eq!(results.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}
