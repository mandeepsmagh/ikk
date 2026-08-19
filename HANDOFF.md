# HANDOFF — S-Tier Refactor: Cleanup Audit

**Branch:** `refac/core-arch` · **State: core refactor + CLI migration complete and green; audit below is the remaining work**

## What's done (verified green, commit `cbd76a6`)

- Unified `Artifact` pipeline, per-package `bin/<name>/` links, Merkle lockfile — all landed.
- Config round-trip fixed: `Config::load` reads `[packages.<name>]` (the shape `save()` writes); legacy top-level `[name]` sections still accepted for now.
- `package_mode` checks `is_local_uri` on the raw URI before `expand_uri` (which mangled multi-slash local paths into `https://` URLs on Windows).
- Flaky Windows junction-remove test fixed (`PermissionDenied` → `cmd rmdir` fallback in `remove_dir_or_link`).
- Full CLI smoke pass exercised: install (local dir), list, info, check, run, sync, upgrade, gc, remove, init.
- `cargo test --workspace` green, clippy 0 new warnings (1 pre-existing on HEAD: needless borrow in `ops.rs:179`), fmt clean.

## Audit: dead / legacy code to remove

1. **Legacy top-level `[name]` package sections in `Config::load`** (`ikk-core/src/config.rs`).
   `save()` only ever writes `[packages.x]`; the loop treating unknown top-level
   sections as packages is pre-refactor format. Remove the loop, the
   `typo_section_gives_clear_error` behavior/test, and the
   `deserialize_top_level_packages` test (rewrite it in nested shape).
   ~15 lines + 2 tests.

2. **`LockedPackage.is_dir`** (`ikk-core/src/lock.rs:43`).
   Always `true`, hardcoded at every insert site. Delete field + the
   `old_lock_without_is_dir_loads` compat test that exists only for it.

3. **`Store::remove(name, version, entry_name)`** (`ikk-core/src/store.rs:210`).
   Two params are literally `_name`/`_version` (unused). Callers should use
   `remove_by_entry` directly; delete the wrapper.

4. **`PackageConfig.sha256` / `--sha256` flag** — stored in config and lockfile,
   printed by `info`/`list`, but **never verified anywhere**. A promise the code
   doesn't keep. Preferred: wire verification into `install_from_source`
   (`ikk-core/src/ops.rs`) — compare `artifact.archive_hash` against
   `pkg.sha256` when set, ~5 lines. Alternative: delete field + flag + prints.

5. **`ikk self-update`** (`ikk-cli/src/commands/self_update.rs`) — half-finished:
   installs ikk into the store under `SELF_BINARY` but never registers it in
   config, and locks "ikk" against itself. Decide: finish or delete. (Owner to decide.)

## Not S-tier yet — honest gaps (bigger than the audit above)

- **No concurrency safety.** No lock around store mutations; two `ikk install`
  processes can race. The `AlreadyExists` handling in `store.insert` is a
  band-aid, not a protocol.
- **Windows junction fallback silently copies.** When `mklink /J` fails,
  `link_bin` falls back to `copy_dir` and `sync` re-copies on every run (WARN
  observed in smoke pass). "Link" semantics degrade to "copy" with no way to
  tell from `list`/`info`.
- **`extract.rs` is a grab-bag** (ROADMAP §1.D "partial" — extraction lives in
  `Source::fetch`, not a separate processor stage).
- **Hand-rolled date math** in `config.rs` (`days_from_civil` etc.) where
  `chrono`/`time` would be one dep.

## Conventions

- Rust 2024, `cargo fmt` (4-space), clippy pedantic with crate-level allows in `lib.rs`.
- Errors: `thiserror` in `error.rs`; `tracing` for logs.
- Tests use `tempfile::tempdir()`; home layout via `IkkHome::new(path)`.
