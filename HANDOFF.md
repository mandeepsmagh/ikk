# HANDOFF — ikk

**Last session:** 2026-08-24 — fixed `self-update` binary replacement (broken on Windows, reported on macOS); unified all platforms on the rename-aside pattern.

## State

- `v0.8.6` is out (in-sync skip for sync/upgrade — see `f67bd98`).
- **This session (uncommitted):** `ikk self-update` now replaces the running binary via the
  rustup/scoop pattern on **all** platforms (single code path, `self_update.rs`):
  1. write new bytes to `{exe}.new` (0755 on Unix),
  2. rename running exe → `{exe}.old` (lock/inode follows the file; path freed),
  3. rename `{exe}.new` → `{exe}`; on failure, best-effort restore from `{exe}.old`,
  4. best-effort delete `{exe}.old`; `sweep_stale_update_files()` at startup
     (`main.rs`) removes `{exe}.old`/`{exe}.new` left by the previous run.
  - Old code renamed *over* the running exe (Unix) or renamed it aside then *into* the
    locked path (Windows) — both hit sharing violations (Windows error 32) when the exe
    is locked, leaving a stray `ikk.new` and a stale/broken install.
  - Regression test: `replace_binary_succeeds_while_exe_is_locked` (CLI).
- Gates green locally: `cargo fmt`, `clippy --all-targets -D warnings`, `cargo test -p ikk-cli` (12).

## Next session

- **Verify on real machines before releasing** (not testable from the dev box):
  - Windows: `ikk self-update` while `ikk.exe` is locked; confirm no `ikk.old`/`ikk.new` leftovers after next start.
  - macOS: same — the user reported the breakage there; the fix is the same code path but untested on darwin.
- Then bump to `0.8.7` + tag (release workflow builds 6 assets + e2e gate).

## Broken boundaries / known flakes

- Pre-existing flakes on Windows (fail on clean `main` too, unrelated to this change):
  `ikk-core` `ops::tests::remove_unlinks_and_cleans` and `ops::tests::link_executables_creates_symlinks`
  (symlink-dependent; pass on Linux CI).
- `ikk check` verifies copied (non-symlink) binaries only by existence — the store hash covers the source, but a tampered copy is not individually re-hashed.
- macOS `bsdtar` adds AppleDouble `._*` entries to local tarballs created without `COPYFILE_DISABLE=1`; those extra entries defeat `unwrap_single_root`. Real upstream release tarballs are unaffected (Linux CI).
- Deferred (unchanged): the pre-publish e2e gate pins `ripgrep@14.1.1`; if that asset disappears, retarget the step to `ikk install ikk --uri mandeepsmagh/ikk`.
