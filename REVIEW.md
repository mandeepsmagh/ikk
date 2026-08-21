# REVIEW

## S-tier Review (2026-07-11, full codebase)

**Verdict:** A+ / near-S-tier. Architecture, testing (57 core tests), and security fundamentals are production-grade. Gaps are all in the seams — a day or two of work, nothing structural.

### Gaps (fix to reach S-tier)

1. **Dead code: `AuthConfig`** — `config.auth` (tokens, `ssh_key`, `ssh_passphrase_env`) in `ikk-core/src/config.rs` is parsed and serialized but used by nothing. Only `RemoteConfig.auth_env` is a real auth path, and the `auth` section has no CLI surface. Either wire it up or delete it.
2. **`self_update` checksum is not fail-closed** — `ikk-cli/src/commands/self_update.rs`: if `SHA256SUMS` is missing or the fetch fails, it prints "skipping verification" and installs anyway. A MITM that strips the checksum file gets a free pass. Make missing checksums a hard error (or require `--insecure` to proceed).
3. **`sync --dry-run` is a lie** — `ikk-cli/src/commands/sync.rs`: prints "would sync X" for every package without checking whether it's outdated, and never reports what `remove_stale` would remove. A dry run must answer "what will change?"
4. **`upgrade` aborts on first failure** — `ikk-cli/src/commands/upgrade.rs`: first error stops the whole loop via `?`, no summary. `sync` collects failures and reports at the end; `upgrade` should match that behavior.
5. **`gc` skips the store lock** — `ikk-cli/src/commands/gc.rs`: deletes store entries via `Ctx::load_readonly`. Concurrent install could link an entry gc just deleted. Use `Ctx::load`.
6. **Stale hardcoded user agent** — `ikk-core/src/remote.rs` `get_json` sets `USER_AGENT: "ikk/0.7"` while `Ctx` builds a client with the real `CARGO_PKG_VERSION`. Two sources of truth; the hardcoded one is stale.
7. **`run.rs` executable heuristic** — `is_executable` (no dots = executable on Unix) misfires on `LICENSE`, `Makefile`, etc. A mode-bit check would be correct.
8. **`config get/set` coverage** — only knows `defaults.remote` and `security.min_release_age_days`. `defaults.self_update_repo` isn't settable via CLI even though docs tell users to "edit that one line in ikk.toml".
9. **`install.ps1` uses `Invoke-WebRequest`** — deprecated in PowerShell 7+; prefer `curl.exe` or `Invoke-RestMethod`.

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
