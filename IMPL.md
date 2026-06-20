# ikk — Staged Implementation Plan

Each stage in its own branch. Every stage produces a working binary.

---

## Stage 0 — Scaffold

**Branch:** `stage-0-scaffold`

Goal: project skeleton, config/deserialization, directory layout, error types. No network, no fs ops beyond `mkdir`.

| #   | Feature | Details |
|-----|---------|---------|
| 0.1 | Cargo workspace | `ikk-core` + `ikk-cli` crates, dependencies locked |
| 0.2 | `IkkHome` | `~/.ikk/` root, `bin/`, `store/`, `stage/` subdirs; `init_dirs()` |
| 0.3 | `ConfigFile` struct | Top-level TOML with `#[serde(flatten)] packages: BTreeMap<String, PackageConfig>` — package names are top-level keys, no `packages.` prefix |
| 0.4 | `PackageConfig` | `uri: String`, `version: Option<String>`, `variant: Option<String>`, `build: Option<Vec<String>>`, `binary: Option<String>`, `sha256: Option<String>` |
| 0.5 | Reserved config keys validation | Reject packages named `defaults`, `security`, `auth`, `store`, `remotes` |
| 0.6 | `Mode` enum + classifier | `Remote`, `LocalBinary`, `LocalBuild` — determined from `uri` scheme + `build` presence |
| 0.7 | `IkkError` + `Result<T>` | All error variants needed by future stages stubbed |
| 0.8 | `LockFile` struct | `packages: BTreeMap<String, LockedPackage>` with `tree_root` Merkle root |
| 0.9 | `LockedPackage` | `version`, `variant`, `uri`, `sha256`, `bin_entry`, `is_dir`, `installed_at` |
| 0.10 | CLI entrypoint | Clap with all subcommands stubbed (`install`, `remove`, `upgrade`, `list`, `sync`, `init`, `check`, `run`, `config`, `gc`, `info`, `uninstall`, `self-update`) |

**Acceptance:** `cargo build && cargo test` passes. `ikk --help` shows all commands.

---

## Stage 1 — Install Core (Forge Discovery)

**Branch:** `stage-1-install-forge`

Goal: `ikk install <owner/repo>` works end-to-end. Forge API discovery, download, verify, store, link, lock.

| #   | Feature | Details |
|-----|---------|---------|
| 1.1 | `RemotesRegistry` | Built-in `remotes.toml` (GitHub, GitLab, Codeberg, Gitea) + user overrides |
| 1.2 | `ConfiguredRemote` | JSON path extraction, auth via env tokens, `latest()` and `assets(version)` |
| 1.3 | URI expansion | `owner/repo` → `https://{defaults.remote}/owner/repo`; full `https://host/owner/repo` stays as-is |
| 1.4 | `Platform` | `current()` detection, `score_asset()` for OS/arch matching |
| 1.5 | `best_asset()` | Scores assets against current platform, returns best match |
| 1.6 | Download | `reqwest` GET, streaming to memory |
| 1.7 | SHA-256 verify | Compute archive + binary hash; if user pinned `sha256` in config, abort on mismatch |
| 1.8 | Archive extraction | `.tar.gz`, `.tar.xz`, `.zip` — extract to `stage/`, locate binary via `name_match_score` + `exe_score` fallback |
| 1.9 | Binary hash | SHA-256 of extracted binary |
| 1.10 | `Store::insert()` | Content-addressed: `store/{hash12}-{name}-{version}/bin/{name}` + `meta.toml`; seal (0555) |
| 1.11 | `Store::remove()` | Unseal + delete directory |
| 1.12 | Symlink in `bin/` | `bin/{name}` → `store/{hash12}-{name}-{version}/bin/{name}` |
| 1.13 | Lock write | Insert `LockedPackage`, recompute `tree_root`, atomic write |
| 1.14 | CLI `ikk install` | `ikk install <name> --uri <uri> [--version] [--binary] [--sha256]` |
| 1.15 | Integration test | Real GitHub download of `BurntSushi/ripgrep` pinned to `14.1.1` |

**Acceptance:** `ikk install rg --uri BurntSushi/ripgrep --version 14.1.1 --binary rg` → binary at `bin/rg` works. `ikk.lock` valid. `ikk check` verifies.

---

## Stage 2 — URL Template Mode

**Branch:** `stage-2-template`

Goal: arbitrary download URLs with `{version}` and `{variant}` tokens. No forge API needed.

| #   | Feature | Details |
|-----|---------|---------|
| 2.1 | `{version}` substitution | If URI contains `{version}`, substitute from `--version` flag (required in this mode) |
| 2.2 | `{variant}` substitution | If URI contains `{variant}`, substitute from `variant` field or `--variant` flag |
| 2.3 | Mode dispatch | `classify(uri)` → `UrlTemplate` if contains tokens, else `ForgeDiscovery` |
| 2.4 | Template download | Direct HTTP GET to resolved URL, no API calls |
| 2.5 | Test: URL template | Install from a direct download URL with `{version}` token |

**Acceptance:** URL templates work. Dispatched cleanly alongside forge discovery mode.

---

## Stage 3 — Local Modes + Variants

**Branch:** `stage-3-local-variants`

Goal: `file://` URIs for local binaries and builds. Variant support end-to-end.

| #   | Feature | Details |
|-----|---------|---------|
| 3.1 | Local binary mode | `uri = "file:///path/to/binary"` — link as-is, no copy, no build |
| 3.2 | Local build mode | `uri = "file:///path/to/source"` + `build = ["cmd1", "cmd2"]` — arbitrary shell commands |
| 3.3 | Build error handling | If any build command fails, abort with exit code and stderr |
| 3.4 | Build output discovery | Scan `target/release/`, `build/`, root dir for binaries; `--binary` to override |
| 3.5 | `variant` in `PackageConfig` | Deserialize `variant: Option<String>` |
| 3.6 | `{variant}` in URL template | Already done in 2.2; now test with real multi-variant packages |
| 3.7 | Variant in store naming | `store/{hash12}-{name}-{version}-{variant}/` |
| 3.8 | Variant in lock file | `LockedPackage.variant: Option<String>` |
| 3.9 | `ikk install --variant` | Override config variant from CLI |
| 3.10 | Multiple variants of same package | `llama-cpp` (cpu) + `llama-cpp-cuda` (cuda12) — different store entries, different names |
| 3.11 | Test: local binary | `file://` URI, symlink created, binary works |
| 3.12 | Test: local build | `file://` URI + `build` commands, build runs, binary installed |
| 3.13 | Test: variant switching | Install cpu variant, then cuda variant of same package |

**Acceptance:** Local modes work. Variants flow through config → store → lock → CLI.

---

## Stage 4 — Multi-File Packages + `ikk run`

**Branch:** `stage-4-directory-packages`

Goal: packages that ship multiple binaries. Directory symlinks. `ikk run`.

| #   | Feature | Details |
|-----|---------|---------|
| 4.1 | `is_dir` detection | Archive with >1 binary → `is_dir = true` in lock |
| 4.2 | Directory extraction | Extract full archive tree to `store/{hash12}-{name}-{version}/` |
| 4.3 | Directory symlink | `bin/{name}` → `store/{hash12}-{name}-{version}/` (directory symlink, not file) |
| 4.4 | `ikk run <name> <binary> [-- args]` | Resolve `bin/{name}/` → find `<binary>` inside → exec with args |
| 4.5 | `ikk list` per-package | Show all binaries inside a directory package |
| 4.6 | Partial `$PATH` behavior | Directory packages are NOT on `$PATH` (only `bin/{name}` is a dir symlink); use `ikk run` |
| 4.7 | Test: multi-binary archive | Install package with 3+ binaries, verify all accessible via `ikk run` |

**Acceptance:** Directory packages work. `ikk run` executes arbitrary binaries within them.

---

## Stage 5 — Windows + Polish

**Branch:** `stage-5-windows-polish`

Goal: full Windows support. UX polish. Remaining CLI commands.

| #   | Feature | Details |
|-----|---------|---------|
| 5.1 | `.bat` shim for single binaries | `bin\{name}.bat` with `@"%~dp0\..\store\{entry}\bin\{name}.exe" %*` |
| 5.2 | NTFS junction for directories | `bin\{name}` → junction → `store\{entry}` (no elevation required) |
| 5.3 | `.exe` handling | Auto-append `.exe` when creating shims; detect `.exe` in archives |
| 5.4 | Platform-conditional linking | Unix: symlink | Windows: shim/junction — same `create_bin_link` interface |
| 5.5 | `ikk sync` | Declarative: read config → diff against lock → install missing, upgrade changed, remove stale |
| 5.6 | `ikk upgrade` | Upgrade all or specific packages to latest |
| 5.7 | `ikk remove` | Remove package, bin link, lock entry, store entry (skip if other packages reference same store hash) |
| 5.8 | `ikk init` | `~/.ikk/` setup, shell integration (bash, zsh, fish, nushell, PowerShell) |
| 5.9 | `ikk uninstall` | Remove `~/.ikk/` entirely, remove shell integration markers |
| 5.10 | `ikk config get/set` | `defaults.remote`, `security.min_release_age_days` |
| 5.11 | `ikk self-update` | Download latest `ikk` from GitHub releases, replace current binary |
| 5.12 | `ikk gc` | Remove store entries not referenced in lock; clean `stage/` leftovers |
| 5.13 | `ikk info` | Show package details, install date, binary path, hashes |
| 5.14 | Release age guard | Port `SecurityConfig` and `min_release_age_days` |
| 5.15 | `ikk check` | Merkle root verify + all binaries re-hash |
| 5.16 | Colored output | Human-readable with colors, spinners for downloads |
| 5.17 | CI | GitHub Actions: build + test on ubuntu, macos, windows |

**Acceptance:** All commands work on all three platforms. CI green.

---

## Complete Checklist

```
Stage 0 — Scaffold
[ ] 0.1   Cargo workspace
[ ] 0.2   IkkHome dirs
[ ] 0.3   ConfigFile with #[serde(flatten)]
[ ] 0.4   PackageConfig fields
[ ] 0.5   Reserved name validation
[ ] 0.6   Mode enum
[ ] 0.7   IkkError
[ ] 0.8   LockFile struct
[ ] 0.9   LockedPackage fields
[ ] 0.10  CLI entrypoint

Stage 1 — Install Core (Forge Discovery)
[ ] 1.1   RemotesRegistry
[ ] 1.2   ConfiguredRemote
[ ] 1.3   URI expansion
[ ] 1.4   Platform detection
[ ] 1.5   best_asset scoring
[ ] 1.6   Download
[ ] 1.7   SHA-256 verify
[ ] 1.8   Archive extraction
[ ] 1.9   Binary hash
[ ] 1.10  Store::insert
[ ] 1.11  Store::remove
[ ] 1.12  Symlink in bin/
[ ] 1.13  Lock write
[ ] 1.14  CLI ikk install
[ ] 1.15  Integration test

Stage 2 — URL Template Mode
[ ] 2.1   {version} substitution
[ ] 2.2   {variant} substitution
[ ] 2.3   Mode dispatch
[ ] 2.4   Template download
[ ] 2.5   Test: URL template

Stage 3 — Local Modes + Variants
[ ] 3.1   Local binary mode
[ ] 3.2   Local build mode
[ ] 3.3   Build error handling
[ ] 3.4   Build output discovery
[ ] 3.5   variant in PackageConfig
[ ] 3.6   {variant} in URL template
[ ] 3.7   Variant in store naming
[ ] 3.8   Variant in lock file
[ ] 3.9   ikk install --variant
[ ] 3.10  Multiple variants
[ ] 3.11  Test: local binary
[ ] 3.12  Test: local build
[ ] 3.13  Test: variant switching

Stage 4 — Multi-File Packages + ikk run
[ ] 4.1   is_dir detection
[ ] 4.2   Directory extraction
[ ] 4.3   Directory symlink
[ ] 4.4   ikk run
[ ] 4.5   ikk list per-package
[ ] 4.6   PATH behavior
[ ] 4.7   Test: multi-binary

Stage 5 — Windows + Polish
[ ] 5.1   .bat shim
[ ] 5.2   NTFS junction
[ ] 5.3   .exe handling
[ ] 5.4   Platform-conditional linking
[ ] 5.5   ikk sync
[ ] 5.6   ikk upgrade
[ ] 5.7   ikk remove
[ ] 5.8   ikk init
[ ] 5.9   ikk uninstall
[ ] 5.10  ikk config
[ ] 5.11  ikk self-update
[ ] 5.12  ikk gc
[ ] 5.13  ikk info
[ ] 5.14  Release age guard
[ ] 5.15  ikk check
[ ] 5.16  Colored output
[ ] 5.17  CI
```
