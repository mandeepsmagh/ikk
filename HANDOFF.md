# HANDOFF — ikk

**Last session:** 2026-08-24 — fixed `self-update` writing the release archive as the binary (the macOS "broken after self-update" report).

## State

- `v0.8.6` is out (sync/upgrade in-sync skip — see `f67bd98`).
- `d6765ff` (committed): `self-update` binary replacement switched to the rename-aside pattern (rustup/scoop) on all platforms. Still unverified on real Windows/macOS.
- **This session (uncommitted):** `self_update.rs` now extracts the `ikk`/`ikk.exe` binary out of the downloaded `.tar.gz`/`.zip` release asset before `replace_binary`. Previously it wrote the archive bytes directly as the executable, so a "successful" self-update left a broken `ikk` — the real cause of the macOS failure (present in both the old rename-over and the new rename-aside paths).
  - Regression test `extract_binary_unpacks_release_archive` added; `flate2`/`tar` added as `ikk-cli` dev-deps for it.
- Gates green locally: `cargo fmt`, `clippy --all-targets -D warnings`, `cargo test` (69 core + 13 CLI + 1 real-world).

## Next session

- Commit this; verify real `ikk self-update` on macOS and Windows before trusting it; then bump + tag `0.8.7`.

## Broken boundaries / known flakes

- `ikk check` verifies copied (non-symlink) binaries only by existence — the store hash covers the source, but a tampered copy is not individually re-hashed.
- macOS `bsdtar` adds AppleDouble `._*` entries to local tarballs created without `COPYFILE_DISABLE=1`; those extra entries defeat `unwrap_single_root`. Real upstream release tarballs are unaffected (Linux CI).
- Pre-existing Windows flake: `ops::tests::{remove_unlinks_and_cleans, link_executables_creates_symlinks}` (symlink-dependent; pass on Linux CI).
- Deferred (unchanged): the pre-publish e2e gate pins `ripgrep@14.1.1`; if that asset disappears, retarget the step to `ikk install ikk --uri mandeepsmagh/ikk`.
