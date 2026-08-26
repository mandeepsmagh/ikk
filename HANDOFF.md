# HANDOFF

## Status

- **v0.8.9** (latest tag). Content-based PATH-linking classifier is complete and committed (`333c622`, `af40470`).
- **P0 item 1 (symlink containment) DONE** — `ops::is_within_root` filters escaping symlink executables at PATH-export and `ikk run`; 5-case matrix test added.
- **P0 item 2 (ZIP path containment) DONE** — `processor.rs` now uses `ZipFile::enclosed_name()` (Windows-aware) + a `starts_with(out_dir)` assert; `safe_join` removed. 5-case traversal test matrix + nested-layout regression test.
- **Classifier v2 landed** — `binary.rs` rewritten (was `binaryv1.rs`): allocation-free `classify(bytes, path)` → `Classification{Format,Role,Architecture}`, cross-host ELF/Mach-O/PE parsing, bounded metadata views + checked arithmetic, `CLASSIFIER_VERSION = 1`; `is_runnable` kept as a compat wrapper. fmt/clippy clean; 110 core + 13 CLI + 1 real-world tests pass on macOS host.
- **Stage 0 architecture review COMPLETE.** Full findings, evidence, and the agreed implementation plan are in `REVIEW.md` (top section, "Staged Hardening").

## Next

1. **P0 item 3** from `REVIEW.md`: atomic store commit + full-hash validation on hit (one change in `store.rs`).
2. Then P1 items (transactional linking, duplicate-basename rejection, host OS/arch validation, ZIP unix modes) in REVIEW.md order.
3. Full plan + invariants + code map are in REVIEW.md — read that section first; do not re-derive from source.

## Gotchas / decisions needed

- **Classification persistence DEFERRED (decision recorded in REVIEW.md).** Do not add `CachedClassification` to CAS metadata — CAS always holds the bytes, so recompute is one bounded local read away, and `meta.content_sha256` is already the integrity anchor (a cache adds no safety). If the install-time double read (hash + classify) ever matters, use an *in-memory* pass-through from `store::insert` → `link_executables`, not a disk cache. `CLASSIFIER_VERSION` stays as the future hook. Revisit only if a read-side consumer appears (`ikk info --verbose`, wrong-arch warnings).
- **Deferred items need explicit user decision before touching** (they invalidate persisted state): mode bits in tree hash (Stage 8.1) and `PACKAGE_DIR`/`bin_entry` renames (Stage 10).
- **Symlink containment uses `std::fs::canonicalize`** — the "depth cap" is the OS's `ELOOP` limit, not a manual one; broken/cycle links fail closed via `canonicalize`'s `Err`.
- **Known pre-existing boundary (NOT fixed by P0 item 1):** `run.rs::find_binary` and `list_binaries` use `path.is_dir()` (follows symlinks) and can recurse forever on a symlink-dir cycle (`bin/loop -> ..`); `collect_executables` already guards this by treating symlink dirs as leaves. Escaping *results* are rejected by `is_within_root`, but the recursion still happens. Decide later whether to switch these to `symlink_metadata` leaf semantics.
- Symlink containment is tested on macOS here; Linux and Windows (Developer Mode symlinks) still need real-machine verification before release. Mach-O/ELF classifier branches and exec-bit behavior likewise.
- ZIP containment relies on `zip::ZipFile::enclosed_name()` (typed-path based). Its behavior for `/absolute` and `C:\absolute` is **strip-and-sanitize** (extracted inside the root), not reject; `../`/`..\` forms are rejected. The `starts_with(out_dir)` check is defense-in-depth.
- `zip` crate 8.6.0 confirmed available with `enclosed_name()` + `unix_mode()` — no dependency changes needed for P0/P1.
