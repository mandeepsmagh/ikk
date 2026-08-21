# HANDOFF — ikk

**Last session:** 2026-07-11 — full S-tier code review. All code read, fmt/clippy/tests green (57 core tests pass). No code changes this session — review only.

## State

- `origin/main` at `17cdfa8`. Tag `v0.8.0` → `65303f3`.
- §1–§4 of `ikk-core/ROADMAP.md` complete. New **§5** added: the 9 S-tier review gaps.
- Full gap list with file locations: **`REVIEW.md`** (top section "S-tier Review").

## Next session: fix the §5 gaps

Work through `REVIEW.md` → "S-tier Review" → Gaps 1–9. Suggested order (independent, any order works):

1. Delete or wire up `AuthConfig` (`ikk-core/src/config.rs`) — decide: delete is the default (nothing uses it).
2. Fail-closed `self_update` checksum (`ikk-cli/src/commands/self_update.rs`) — missing/unfetchable `SHA256SUMS` → hard error unless `--insecure`.
3. Honest `sync --dry-run` (`ikk-cli/src/commands/sync.rs`) — compare lock vs config, report what would install/upgrade/remove.
4. `upgrade` failure summary (`ikk-cli/src/commands/upgrade.rs`) — collect errors like `sync` does, report at end.
5. `gc` takes store lock (`ikk-cli/src/commands/gc.rs`) — `load_readonly` → `load`.
6. Stale `USER_AGENT "ikk/0.7"` in `ikk-core/src/remote.rs` `get_json` — remove header (client already sets it) or use `CARGO_PKG_VERSION`.
7. `run.rs` `is_executable` — mode-bit check on Unix instead of "no dots" heuristic.
8. `config get/set` — add `defaults.self_update_repo`.
9. `install.ps1` — replace `Invoke-WebRequest` with `curl.exe` (present on Win10+/PS7).

Verify: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`. Update `REVIEW.md` as items close. When all 9 are done, delete this file.

## Broken boundaries / known flakes

- None. Build, clippy, tests all green at `17cdfa8`.
- Live `self-update` e2e still blocked: repo private → GitHub API 404 without token. Not part of §5.
