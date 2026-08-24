# HANDOFF — ikk

**Last session:** 2026-08-24 — `v0.8.8`: link only `bin/` + root executables (Nix-style), fixes PATH pollution.

## State

- `v0.8.8` tagged (this session). Since `v0.8.7`:
  - `a2419eb` — `link_executables` exposes only executables at the package root or inside a `bin/` dir (`is_path_exported`). Neovim's executable `*.so` parsers and `less.sh` (under `lib/`/`share/`) no longer pollute `~/.ikk/bin`. Regression test `link_executables_skips_lib_and_share`.
- `v0.8.7` carried the self-update fixes: rename-aside swap (`d6765ff`), extract binary from release archive (`643d59d`), copy-fallback removal (`5f24e58`).
- Still unverified on a real machine (not reproducible in CI):
  - macOS: full `ikk self-update` replace — owner is testing (reinstall `0.8.8` via `install.sh`, then future self-updates use the fixed code).
  - Windows: `ikk self-update` while `ikk.exe` is actually locked. GitHub Windows runners have Developer Mode, so the file-symlink fallback is not exercised in CI either.
- Gates green locally: `cargo fmt`, `clippy --all-targets -D warnings`, `cargo test` (71 core + 13 CLI + 1 real-world).

## Bootstrap caveat (important)

- Self-update runs the **currently installed** binary's code. Installs `<= 0.8.6` still have the tarball bug, so upgrading via `ikk self-update` would brick them. Reinstall once via `install.sh`, then self-update is safe going forward.

## Next session

- Owner confirms macOS self-update test; then Windows real-machine test.

## Broken boundaries / known flakes

- `ikk check` verifies copied (non-symlink) binaries only by existence — the store hash covers the source, but a tampered copy is not individually re-hashed.
- macOS `bsdtar` adds AppleDouble `._*` entries to local tarballs created without `COPYFILE_DISABLE=1`; those extra entries defeat `unwrap_single_root`. Real upstream release tarballs are unaffected (Linux CI).
- Deferred (unchanged): the pre-publish e2e gate pins `ripgrep@14.1.1`; if that asset disappears, retarget the step to `ikk install ikk --uri mandeepsmagh/ikk`.
