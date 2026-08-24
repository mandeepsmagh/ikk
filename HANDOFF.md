# HANDOFF — ikk

**Last session:** 2026-08-24 — shipped `v0.8.8`; identified that PATH-linking needs a **content-based** binary classifier (not name/location heuristics). Decision deferred to next session.

## State

- `v0.8.8` tagged. Since `v0.8.7`: `link_executables` exposes only executables at the package root or inside a `bin/` dir (`is_path_exported`), so neovim's `lib/*.so` + `share/less.sh` no longer leak into `~/.ikk/bin`. Test: `link_executables_skips_lib_and_share`.
- `v0.8.7` carried the self-update fixes: rename-aside swap (`d6765ff`), extract binary from release archive (`643d59d`), copy-fallback removal (`5f24e58`).
- Still unverified on real machines (not reproducible in CI): macOS `ikk self-update` replace; Windows `ikk self-update` with a truly locked `ikk.exe` (GH Windows runners have Developer Mode, so the file-symlink fallback is never exercised).
- Gates green locally: `cargo fmt`, `clippy --all-targets -D warnings`, `cargo test` (71 core + 13 CLI + 1 real-world).

## Bootstrap caveat (important)

- Self-update runs the **currently installed** binary's code. Installs `<= 0.8.6` still write the release archive as the executable, so upgrading via `ikk self-update` would brick them. Reinstall once via `install.sh`; then self-update is safe.

## Next session — holistic PATH-linking classifier (decide + implement)

**Problem (why the current rule is insufficient):** the executable bit is a bad proxy — shared libraries ship with `+x`. Evidence already reproduced:
- neovim: `lib/nvim/parser/*.so` and `share/nvim/runtime/scripts/less.sh` are `0755` → leaked (fixed by `is_path_exported`).
- llama.cpp macOS (`llama-b10605-bin-macos-arm64.tar.gz`): tarball is **flat** — binaries *and* `libggml*.dylib`/`libllama*.dylib` sit together at the package root, all `0755` → root-level `.dylib` still leak. A `.dylib`/`.so` extension denylist was tried and rejected by the owner as whack-a-mole.

**Proposed holistic fix (one rule, no per-package knowledge):** decide runnability from **file content**, not name/location:
1. Starts with `#!` → script → runnable.
2. Mach-O → only file type `MH_EXECUTE` (`0x2`); skip `MH_DYLIB` (`0x6`), `MH_BUNDLE` (`0x8`), others.
3. ELF → `e_type == ET_EXEC` (`2`), or `ET_DYN` (`3`) **with a `PT_INTERP` program header** (distinguishes PIE executables from `.so`).
4. Windows → unchanged `.exe`/`.bat`/`.cmd` (extension-based; no exec-bit concept).

Keep one location guard (applies to all packages): link only from `bin/` dirs or the package root; never `lib/`/`share/`/elsewhere.

**Code locations:**
- `ikk-core/src/ops.rs`: `link_executables`, `is_path_exported`, `collect_executables`, `is_executable` (mode bits on Unix / extensions on Windows).
- `ikk-cli/src/commands/run.rs`: uses `ikk_core::ops::{collect_executables, is_executable}` for `single_executable`/`list_binaries` — **must use the same content classifier** so `ikk run`'s "sole executable" fallback never picks a `.so`/`.dylib`.
- Consider a tiny new core module (e.g. `ikk-core/src/binary.rs`) exposing one predicate, shared by `ops` and `run`.

**Open questions to settle while implementing:**
- Should `bin/` entries also be content-checked, or trusted by convention? (Trusting `bin/` is simpler; content-checking is stricter. Recommend: content-check everywhere for one consistent rule.)
- `.so` with `PT_INTERP` (e.g. libc) classifies as runnable — acceptable? These don't appear at package roots in practice.
- Hand-roll header parsing (~60 lines, no new deps) vs adding `object`/`goblin`. Recommend hand-roll for just the two fields.
- Static ELF binaries (`ET_EXEC`) and macOS fat binaries are both covered by the above; add tests for each.

## Broken boundaries / known flakes

- `ikk check` verifies copied (non-symlink) binaries only by existence — the store hash covers the source, but a tampered copy is not individually re-hashed.
- macOS `bsdtar` adds AppleDouble `._*` entries to local tarballs created without `COPYFILE_DISABLE=1`; those extra entries defeat `unwrap_single_root`. Real upstream release tarballs are unaffected (Linux CI).
- Deferred (unchanged): the pre-publish e2e gate pins `ripgrep@14.1.1`; if that asset disappears, retarget the step to `ikk install ikk --uri mandeepsmagh/ikk`.
