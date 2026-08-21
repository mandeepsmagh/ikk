# REVIEW

## S-tier Review (2026-07-11, full codebase) — closed 2026-07-11

**Verdict:** A+ / near-S-tier. Architecture, testing (57 core tests), and security fundamentals are production-grade. Gaps were all in the seams — a day or two of work, nothing structural.

### Gaps (all fixed)

1. ✅ **Dead code: `AuthConfig`** — deleted from `ikk-core/src/config.rs`. `RemoteConfig.auth_env` remains the only auth path.
2. ✅ **`self_update` checksum is now fail-closed** — `SHA256SUMS` missing/unfetchable or asset absent from the file → hard error; `--insecure` opts out with a warning.
3. ✅ **`sync --dry-run` answers "what will change?"** — reports would install / reinstall (config changed) / upgrade (lock vs resolved version) / remove (stale), errors collected like a real run.
4. ✅ **`upgrade` collects failures** — one broken package no longer aborts the rest; summary printed at the end, non-zero exit on any failure.
5. ✅ **`gc` holds the store lock** — `Ctx::load` for the destructive path, `load_readonly` for `--dry-run`.
6. ✅ **User agent from `CARGO_PKG_VERSION`** — `remote.rs` `get_json` no longer hardcodes `ikk/0.7`.
7. ✅ **`run.rs` mode-bit check** — Unix `is_executable` checks `0o111` mode bits (plus `is_file`) instead of the no-dots heuristic; Windows matches `.exe/.bat/.cmd`.
8. ✅ **`config get/set defaults.self_update_repo`** — settable via CLI (validated as `owner/repo`), shown by bare `ikk config`.
9. ✅ **`install.ps1` uses `curl.exe`** — for both the asset and `SHA256SUMS`; `Invoke-RestMethod` kept only for the GitHub API call (not deprecated).

### Deferred (low risk, from prior review)

- Non-test `.unwrap()`/`.expect()` in `processor.rs` `attach_dmg` (`to_str()`) and `registry.rs` (built-in `remotes.toml`). Trusted built-in data.
- No `windows-arm64` / `linux-musl` release assets; self-update reports "no ikk release asset" there.

### Deferred (environment)

- **Live `ikk self-update` e2e** — repo `mandeepsmagh/ikk` is private; `api.github.com/.../releases/latest` returns 404 without a token. Re-run the §4 gate once the repo is public (or with an authenticated API client).

---

## Release Pipeline (closed)

**Date:** after v0.8.0 re-tag (`65303f3`).

1. **`score_asset` x86_64** — `contains` closure now matches separator-containing variants as raw substrings; regression test `score_x86_64_beats_wrong_arch` asserts the matching-arch asset wins.
2. **`install.sh` / `install.ps1`** — rewritten for `ikk-{os}-{arch}.{ext}` naming; verify against published `SHA256SUMS`.
3. **Version mismatch** — crates bumped `0.7.1` → `0.8.0`; tag re-pointed at `65303f3`.
4. **Checksum consistency** — single `SHA256SUMS` file, used by self-update and both install scripts.
5. **§4 gate** — code complete; live run blocked by private-repo 404 (see Deferred above).
6. **README** — routes through the fixed scripts; no change needed.
