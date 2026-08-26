# HANDOFF

## Status

- **v0.8.9** (latest tag). Content-based PATH-linking classifier is complete and committed (`333c622`, `af40470`).
- **P0 item 1 (symlink containment) DONE** — `ops::is_within_root` filters escaping symlink executables at PATH-export and `ikk run`; 5-case matrix test added. fmt/clippy green; 79 core + 13 CLI + 1 real-world tests pass on macOS host.
- **Stage 0 architecture review COMPLETE.** Full findings, evidence, and the agreed implementation plan are in `REVIEW.md` (top section, "Staged Hardening").

## Next

1. **P0 item 2** from `REVIEW.md`: ZIP `enclosed_name()` replaces `safe_join` in `processor.rs` (+ the 5 traversal tests listed there).
2. Then **P0 item 3**: atomic store commit + full-hash validation on hit (one change in `store.rs`).
3. Full plan + invariants + code map are in REVIEW.md — read that section first; do not re-derive from source.

## Gotchas / decisions needed

- **Deferred items need explicit user decision before touching** (they invalidate persisted state): mode bits in tree hash (Stage 8.1) and `PACKAGE_DIR`/`bin_entry` renames (Stage 10).
- **Symlink containment uses `std::fs::canonicalize`** — the "depth cap" is the OS's `ELOOP` limit, not a manual one; broken/cycle links fail closed via `canonicalize`'s `Err`.
- **Known pre-existing boundary (NOT fixed by P0 item 1):** `run.rs::find_binary` and `list_binaries` use `path.is_dir()` (follows symlinks) and can recurse forever on a symlink-dir cycle (`bin/loop -> ..`); `collect_executables` already guards this by treating symlink dirs as leaves. Escaping *results* are rejected by `is_within_root`, but the recursion still happens. Decide later whether to switch these to `symlink_metadata` leaf semantics.
- Symlink containment is tested on macOS here; Linux and Windows (Developer Mode symlinks) still need real-machine verification before release. Mach-O/ELF classifier branches and exec-bit behavior likewise.
- `zip` crate 8.6.0 confirmed available with `enclosed_name()` + `unix_mode()` — no dependency changes needed for P0/P1.
