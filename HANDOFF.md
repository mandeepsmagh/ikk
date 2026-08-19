# HANDOFF — S-Tier Refactor: Cleanup Audit

**Branch:** `refac/core-arch` · **State: audit items 1–4 done and green; item 5 (self-update) needs an owner decision**

## What's done this session (audit items 1–4)

- Removed legacy top-level `[name]` package sections from `Config::load` (only `[packages.<name>]` accepted now); dropped `KNOWN_SECTIONS`, rewrote `deserialize_top_level_packages` as `deserialize_nested_packages`, deleted `typo_section_gives_clear_error`.
- Deleted `LockedPackage.is_dir` + `default_true` + `old_lock_without_is_dir_loads` test. Note: old lock files with an `is_dir = true` line still load fine (serde ignores unknown fields by default).
- Deleted `Store::remove(name, version, entry_name)` wrapper; callers use `remove_by_entry`.
- **sha256 verification now enforced**: `install_from_source` in `ops.rs` compares `artifact.archive_hash` against `pkg.sha256` when set, returning `HashMismatch`. Local *directory* sources have an empty archive hash, so a pinned sha256 on a local dir fails (intended: no archive to verify). The `--sha256` flag / config field / lockfile prints are now kept and honored.
- Green: `cargo test --workspace`, clippy 0 warnings, fmt clean.

## Previously done (verified green, commit `cbd76a6`)

- Unified `Artifact` pipeline, per-package `bin/<name>/` links, Merkle lockfile — all landed.
- Config round-trip fixed: `Config::load` reads `[packages.<name>]` (the shape `save()` writes); legacy top-level `[name]` sections still accepted for now.
- `package_mode` checks `is_local_uri` on the raw URI before `expand_uri` (which mangled multi-slash local paths into `https://` URLs on Windows).
- Flaky Windows junction-remove test fixed (`PermissionDenied` → `cmd rmdir` fallback in `remove_dir_or_link`).
- Full CLI smoke pass exercised: install (local dir), list, info, check, run, sync, upgrade, gc, remove, init.
- `cargo test --workspace` green, clippy 0 new warnings (1 pre-existing on HEAD: needless borrow in `ops.rs:179`), fmt clean.

## Remaining audit item

1. **`ikk self-update`** (`ikk-cli/src/commands/self_update.rs`) — half-finished:
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
