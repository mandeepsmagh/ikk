# HANDOFF

## Status

- **v0.8.9** (latest tag). Content-based PATH-linking classifier is complete and committed (`333c622`, `af40470`); all 73 tests + clippy/fmt green on Windows host.
- **Stage 0 architecture review COMPLETE — no code changed.** Full findings, evidence, and the agreed implementation plan are in `REVIEW.md` (top section, "Staged Hardening"). `ARCH-REVIEW.md` has been deleted; its content lives in REVIEW.md now.

## Next

1. Start **P0 item 1** from `REVIEW.md`: symlink containment at export time in `ops.rs::link_executables` (+ same helper for `ikk run`), with the 5-test matrix listed there.
2. Then P0 items 2–3 (ZIP `enclosed_name()`, atomic store commit + hash validation). One commit per item, fmt/clippy/tests after each.
3. Full plan + invariants + code map are in REVIEW.md — read that section first; do not re-derive from source.

## Gotchas / decisions needed

- **Deferred items need explicit user decision before touching** (they invalidate persisted state): mode bits in tree hash (Stage 8.1) and `PACKAGE_DIR`/`bin_entry` renames (Stage 10).
- Reviewed on Windows; symlink containment, Mach-O/ELF classifier branches, and exec-bit behavior are covered by synthetic tests but **must be re-verified on real Linux/macOS before release** (same as the pre-existing self-update edge cases).
- `zip` crate 8.6.0 confirmed available with `enclosed_name()` + `unix_mode()` — no dependency changes needed for P0/P1.
