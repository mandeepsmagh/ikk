# HANDOFF — ikk

**Last session:** 2026-08-24 — finished the content-based PATH-linking classifier (the item deferred at `v0.8.8`).

## State

- PATH-linking now classifies executables by **file content**, not name/location/exec-bit. `ikk-core/src/binary.rs` (`is_runnable`) is the single predicate, shared by `ops::link_executables`/`collect_executables` and `ikk run` (`single_executable`, `list_binaries`).
  - `#!` → runnable.
  - Mach-O (thin/fat, 32/64-bit, LE/BE) → only `MH_EXECUTE`; skips `MH_DYLIB`/`MH_BUNDLE`/objects. Verified against real macOS binaries: llama.cpp's `lib*.dylib` (MH_DYLIB) and neovim's `parser/*.so` (MH_BUNDLE) are rejected; `llama-cli`/`nvim` (MH_EXECUTE) accepted.
  - ELF64 → `ET_EXEC`, or `ET_DYN` + `PT_INTERP` (the PIE-vs-`.so` distinction). ELF32 fails closed.
  - Windows → unchanged `.exe`/`.bat`/`.cmd`.
- The `bin/`-or-root location guard (`is_path_exported`) is kept as a **secondary** filter — content alone can't reject a runnable script, so the guard is what keeps neovim's `share/.../less.sh` off PATH.
- `ops::is_executable` removed (it mixed exec-bit + content and had no callers left). `binary::is_runnable` is the one rule everywhere.

## Still unverified on real machines (not reproducible in CI)

- macOS `ikk self-update` replace (rename-aside swap).
- Windows `ikk self-update` with a truly locked `ikk.exe` (GH Windows runners have Developer Mode, so the file-symlink fallback is never exercised).

## Bootstrap caveat (important)

- Self-update runs the **currently installed** binary's code. Installs `<= 0.8.6` still write the release archive as the executable, so upgrading via `ikk self-update` would brick them. Reinstall once via `install.sh`; then self-update is safe.

## Broken boundaries / known flakes

- `ikk check` verifies copied (non-symlink) binaries only by existence — the store hash covers the source, but a tampered copy is not individually re-hashed.
- macOS `bsdtar` adds AppleDouble `._*` entries to local tarballs created without `COPYFILE_DISABLE=1`; those extra entries defeat `unwrap_single_root`. Real upstream release tarballs are unaffected (Linux CI).
- The pre-publish e2e gate pins `ripgrep@14.1.1`; if that asset disappears, retarget the step to `ikk install ikk --uri mandeepsmagh/ikk`.
