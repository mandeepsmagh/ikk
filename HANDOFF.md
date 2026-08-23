# HANDOFF — ikk

**Last session:** 2026-08-23 — flat `bin/` executable links + `self_update_repo` defaulting. Implemented, not yet committed.

## State

- **Flat `~/.ikk/bin` layout** replaces the per-package `bin/<name>/` directory link:
  - `ops::link_executables` recursively scans a package's store root for executables and symlinks each into `~/.ikk/bin/<exe>`. Windows uses file symlinks with a `std::fs::copy` fallback (link_type `"copy"`).
  - Name collisions across packages are rejected — the error names the package that already owns the binary.
  - `ikk.lock` records `bins` (exe → path relative to the package root) per package and hashes it into the merkle leaf.
  - `ikk run` resolves the package root from `store/<bin_entry>/bin`; there is no per-package dir anymore.
  - `ikk check` also verifies each `bin/<exe>` symlink points at its store binary.
  - `shell.rs` PATH integration now adds only `~/.ikk/bin`.
- **`self_update_repo` fixed**: `Defaults.self_update_repo` has a serde default, so a config missing the key loads as `mandeepsmagh/ikk`; `ikk init` backfills **and persists** it when the key is absent from an existing config (and never overwrites a user-set value).
- Gates green locally: `cargo fmt`, `clippy --all-targets -D warnings`, `cargo test` (69 core + 10 CLI + 1 real-world).
- `install.sh` / `install.ps1`: **no changes needed** — both already install `ikk` into `~/.ikk/bin` and defer PATH to `ikk init` (ps1 additionally sets User PATH directly, a pre-existing inconsistency).

## Next session

- Commit this change; bump the version if it ships as a release.
- If releasing: this is a breaking layout/lock change vs v0.8.4 — existing installs should re-run `ikk sync` after upgrade. No backward-compat shims required (owner confirmed greenfield).

## Broken boundaries / known flakes

- `ikk check` verifies copied (non-symlink) binaries only by existence — the store hash covers the source, but a tampered copy is not individually re-hashed.
- macOS `bsdtar` adds AppleDouble `._*` entries to local tarballs created without `COPYFILE_DISABLE=1`; those extra entries defeat `unwrap_single_root`. Real upstream release tarballs are unaffected (Linux CI).
- Deferred (unchanged): the pre-publish e2e gate pins `ripgrep@14.1.1`; if that asset disappears, retarget the step to `ikk install ikk --uri mandeepsmagh/ikk`.
