# HANDOFF — ikk

**Last session:** 2026-08-24 — fixed `self-update` (tarball extraction + copy-fallback removal), releasing `v0.8.7`.

## State

- `v0.8.7` tagged (this session). Contains, since `v0.8.6`:
  - `d6765ff` — self-update binary swap via rename-aside on all platforms (rustup/scoop pattern).
  - `643d59d` — self-update extracts the `ikk`/`ikk.exe` binary from the release archive before swapping (previously wrote the archive bytes as the executable — the macOS "broken after self-update" bug).
  - `5f24e58` — `remove_dir_or_link` unlinks plain-file copies (Windows copy fallback), plus the Windows test fix.
- Known unverified (needs real machine, not reproducible in CI):
  - Windows: `ikk self-update` while `ikk.exe` is actually locked (error 32 path). GitHub Windows runners have Developer Mode, so the symlink fallback path is never exercised in CI either.
  - macOS: full `self-update` replace — owner is testing this on a real Mac (reinstall `0.8.7` via `install.sh`, then future self-updates use the fixed code).
- Gates green locally: `cargo fmt`, `clippy --all-targets -D warnings`, `cargo test` (69 core + 13 CLI + 1 real-world).

## Bootstrap caveat (important)

- Self-update runs the **currently installed** binary's code. An install of `<= 0.8.6` still has the tarball bug, so upgrading `0.8.6 → 0.8.7` *via `ikk self-update`* would still brick it. Users on `<= 0.8.6` must reinstall once via `install.sh` (or otherwise place the fixed binary), then self-update is safe going forward.

## Next session

- Owner confirms macOS test; then Windows real-machine test; if both good, self-update is considered verified.

## Broken boundaries / known flakes

- `ikk check` verifies copied (non-symlink) binaries only by existence — the store hash covers the source, but a tampered copy is not individually re-hashed.
- macOS `bsdtar` adds AppleDouble `._*` entries to local tarballs created without `COPYFILE_DISABLE=1`; those extra entries defeat `unwrap_single_root`. Real upstream release tarballs are unaffected (Linux CI).
- Deferred (unchanged): the pre-publish e2e gate pins `ripgrep@14.1.1`; if that asset disappears, retarget the step to `ikk install ikk --uri mandeepsmagh/ikk`.
