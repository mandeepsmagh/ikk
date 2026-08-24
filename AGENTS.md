# AGENTS.md

Standing rules for any agent working in this repository.

## Handoff discipline

- `HANDOFF.md` (repo root) is the entry point for any new session. Read it first, before re-deriving state from source.
- Any commit that changes project state must update `HANDOFF.md` in the **same commit**. Never batch doc updates for later.
- If interrupted mid-task, the last commit must leave `HANDOFF.md` truthful: what's done, what's next, known flakes/broken boundaries.
- When the handoff work is complete, **delete `HANDOFF.md`**. It is scaffolding, not documentation. The permanent record is `ikk-core/ROADMAP.md` + git history + the code.
- Keep `HANDOFF.md` to non-obvious things: decisions and their rationale, broken boundaries (exact signatures), known flakes. Never restate what the code already says.
- Roadmap status: keep the status table at the top of `ikk-core/ROADMAP.md` current (✅/⚠️/⏳ per item) in the same commit as the work it tracks.

## Testing discipline

- **Stop after two inconclusive attempts.** If a behavior can't be verified in this environment (e.g. OS-specific file-locking, cross-platform runtime semantics), say so, record it in `HANDOFF.md` as "needs verification on a real machine", and move on. Do not build escalating test harnesses to force a local proof.
- **Test artifacts go in `tempfile::tempdir()` / `std::env::temp_dir()` with a unique suffix, and are removed at the end of the test.** Never leave experiment binaries/scripts in shared temp dirs across turns; clean up before committing.

## Conventions

- Rust 2024 edition. `cargo fmt` (4-space indent). Clippy pedantic with the crate-level allows in `ikk-core/src/lib.rs`.
- Errors via `thiserror` in `ikk-core/src/error.rs`; logging via `tracing`.
- Tests use `tempfile::tempdir()`; home layout via `IkkHome::new(path)`.
- Commit messages state WIP boundaries explicitly (e.g. "WIP: CLI not yet updated").
