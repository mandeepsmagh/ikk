# HANDOFF

## Status

- **v0.9.0** (latest tag). Full P0+P1 hardening pass shipped (all items below); released from this commit.
- **P0 item 1 (symlink containment) DONE** — `ops::is_within_root` filters escaping symlink executables at PATH-export and `ikk run`; 5-case matrix test added.
- **P0 item 2 (ZIP path containment) DONE** — `processor.rs` now uses `ZipFile::enclosed_name()` (Windows-aware) + a `starts_with(out_dir)` assert; `safe_join` removed. 5-case traversal test matrix + nested-layout regression test.
- **P0 item 3 (atomic store commit + hash validation) DONE** — `store::insert` now populates a `store/.tmp-{pid}-{counter}` dir and atomically `rename`s it into place; on a store hit it reads `meta.toml` and self-heals (remove + repopulate) on missing/mismatched `content_sha256`. Stale `.tmp-*` dirs are swept under the exclusive store lock. 3 new tests (partial-entry self-heal, hash-mismatch self-heal, temp-dir sweep).
- **P1 item 2 (transactional linking) DONE** — `link_executables` reordered to validate collisions before mutating `bin/`; a failed upgrade (e.g. a new bin name colliding with another package) now leaves the old links intact. Test: collision-on-upgrade asserts stale old link survives.
- **P1 items 1.1+1.2 (duplicate-basename rejection + `bin` parent filter) DONE** — `link_executables` rejects two files sharing one basename as ambiguous (lists both paths); `is_path_exported` now only checks *parent* `bin` components, so a runnable file named `bin` under `scripts/` is no longer exported. 2 tests added.
- **P1 item 4.x (host OS/arch validation → exposure filter) DONE** — `link_executables` now takes an explicit `Platform` and links only binaries native to it (`is_host_native`: OS-format match + strict arch match; `Universal` + scripts always). The CAS keeps every binary; cross binaries stay reachable via `ikk run`. 4.3 (fat-Mach-O) and 4.4 (checked arithmetic) were already done by classifier v2. Pure `is_host_native` unit test.
- **P1 item 7.2 (ZIP unix_mode preservation) DONE** — `extract_zip_to_dir` applies `unix_mode() & 0o777` to each extracted entry on unix; synthetic zip test asserts `0755` is preserved. **All P0 + P1 items are now complete.** fmt/clippy clean; 118 core + 13 CLI + 1 real-world tests pass on macOS host.
- **Classifier v2 landed** — `binary.rs` rewritten (was `binaryv1.rs`): allocation-free `classify(bytes, path)` → `Classification{Format,Role,Architecture}`, cross-host ELF/Mach-O/PE parsing, bounded metadata views + checked arithmetic, `CLASSIFIER_VERSION = 1`; the old `is_runnable` wrapper was removed and its two call sites now use `is_command_candidate`. fmt/clippy clean; 110 core + 13 CLI + 1 real-world tests pass on macOS host.
- **Stage 0 architecture review COMPLETE.** Full findings, evidence, and the agreed implementation plan are in `REVIEW.md` (top section, "Staged Hardening").

## Next

1. **P2 items** from `REVIEW.md`, in order: (8.2) streaming file hashing (behavior-preserving), (9) lock integrity root doc/separators, (11) `./`/`../` local paths + `source_url` naming.
2. **Deferred — need explicit user decision first:** mode bits in tree hash (8.1, invalidates all store identities) and `PACKAGE_DIR`/`bin_entry` renames (10, persisted names).
3. Full plan + invariants + code map are in REVIEW.md — read that section first; do not re-derive from source.

## Gotchas / decisions needed

- **`.ikk/bin` is now platform-native-only by design (decision in REVIEW.md).** The CAS keeps every binary; `ikk run` resolves against the store root (not PATH), so cross binaries stay usable. `--target` (explicit cross-platform install) is deferred — the explicit `Platform` parameter + pure `is_host_native(format, arch, platform)` are the hooks (a `--target` would thread one `Platform` through `score_asset` and this filter). Deliberate strictness: x86_64-on-Apple-Silicon (Rosetta) and 32-bit-on-x86_64 (multilib) are not auto-linked; `.bat`/`.cmd` on unix are still linked (classifier folds them into `Script`).

- **Classification persistence DEFERRED (decision recorded in REVIEW.md).** Do not add `CachedClassification` to CAS metadata — CAS always holds the bytes, so recompute is one bounded local read away, and `meta.content_sha256` is already the integrity anchor (a cache adds no safety). If the install-time double read (hash + classify) ever matters, use an *in-memory* pass-through from `store::insert` → `link_executables`, not a disk cache. `CLASSIFIER_VERSION` stays as the future hook. Revisit only if a read-side consumer appears (`ikk info --verbose`, wrong-arch warnings).
- **Deferred items need explicit user decision before touching** (they invalidate persisted state): mode bits in tree hash (Stage 8.1) and `PACKAGE_DIR`/`bin_entry` renames (Stage 10).
- **Symlink containment uses `std::fs::canonicalize`** — the "depth cap" is the OS's `ELOOP` limit, not a manual one; broken/cycle links fail closed via `canonicalize`'s `Err`.
- **Known pre-existing boundary (NOT fixed by P0 item 1):** `run.rs::find_binary` and `list_binaries` use `path.is_dir()` (follows symlinks) and can recurse forever on a symlink-dir cycle (`bin/loop -> ..`); `collect_executables` already guards this by treating symlink dirs as leaves. Escaping *results* are rejected by `is_within_root`, but the recursion still happens. Decide later whether to switch these to `symlink_metadata` leaf semantics.
- Symlink containment is tested on macOS here; Linux and Windows (Developer Mode symlinks) still need real-machine verification before release. Mach-O/ELF classifier branches and exec-bit behavior likewise.
- ZIP containment relies on `zip::ZipFile::enclosed_name()` (typed-path based). Its behavior for `/absolute` and `C:\absolute` is **strip-and-sanitize** (extracted inside the root), not reject; `../`/`..\` forms are rejected. The `starts_with(out_dir)` check is defense-in-depth.
- `zip` crate 8.6.0 confirmed available with `enclosed_name()` + `unix_mode()` — no dependency changes needed for P0/P1.
