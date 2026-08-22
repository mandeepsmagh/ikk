# REVIEW

## macOS release asset fix (2026-08-22)

**Found:** the published macOS assets (arm64 + x86_64) were dynamically linked against Homebrew's `liblzma` (`/opt/homebrew/opt/xz/lib/liblzma.5.dylib`). The GitHub `macos-latest` runner ships Homebrew `xz`, so `lzma-sys` linked against it and baked the Homebrew path into the binary. On any Mac without Homebrew `xz` at that path the binary crashes at launch: `dyld: Library not loaded: /opt/homebrew/opt/xz/lib/liblzma.5.dylib`.

**Fixed:** `xz2` now builds with `features = ["static"]` (compiles liblzma from source, statically linked). Verified: the release binary depends only on macOS system frameworks, runs, and `.tar.xz` extraction still works (new regression test `extracts_tar_xz`).

**Note:** the already-published `v0.8.2` assets still carry the old dynamic link — a new release (`v0.8.3`) is required to ship fixed binaries.

---

## Live self-update e2e (2026-08-22) — closed

Repo made public. Live unauthenticated e2e passes: `ikk self-update --check` → `ikk is up to date (0.8.2)`; asset + `SHA256SUMS` download verified (checksum match) over plain HTTPS. This closes the long-deferred §4 gate.

**Found + fixed one more bug:** `self_update.rs` compared `release.version` (`v0.8.2`) raw against `CARGO_PKG_VERSION` (`0.8.2`), so it always reported an upgrade and would re-download the same version. Now compared via `strip_v()` (strips a leading `v`/`V`).

---

## Fix session (2026-08-22) — all findings addressed

All three high-severity bugs and the four mediums fixed; verified live. Gates green (66 core + 9 CLI + 1 real-world; clippy/fmt clean).

1. ✅ `ikk gc` — only collects directories containing `meta.toml`; skips `.lock` and partial entries (`gc.rs::is_store_entry`).
2. ✅ Symlink false-tamper — `store::copy_dir_contents` now re-creates symlinks instead of dereferencing (matches `hash_dir`), with a cycle-free walk + Windows copy fallback; `processor` reuses it for DMG.
3. ✅ Package-name data loss — `ops::validate_name` rejects empty/`.`/`..`/separators/metacharacters; enforced in `link_bin`/`remove` (core) and `add`/`remove`/`run` (CLI).
4. ✅ `upgrade --force` — drops the pin so pinned packages actually resolve to `latest`.
5. ✅ Bash rc mismatch — `write_rc`/`remove_rc` now resolve `.bash_profile` vs `.bashrc` via `Shell::bash_rc_file`, matching `rc_file()`.
6. ✅ Forge downloads — `RemoteSource::fetch` streams through `progress::download_bytes` (bearer-auth aware) with a progress bar.
7. ✅ `self-update` — expands `self_update_repo` against `defaults.remote`, falling back to `github.com` (no more "relative URL without a base").

Also: dead code removed (`LockFile::diff`/`SyncPlan`, `Store::find_all`), unreachable `check` branch simplified, case-insensitive `sha256` pin comparison, safe `entry_name` slicing, `link_bin`'s `cmd` sweep gated `#[cfg(windows)]`, and `store.insert` cleans up a partial entry on failure.

Still deferred (documented, not bugs): unkeyed Merkle root (accidental-corruption detection only), `install.sh` JSON parsing via `grep|sed`, live `ikk self-update` e2e (private repo), `windows-arm64`/`linux-musl` assets.

---

## S-tier Review (2026-08-22) — full re-review

**Verdict:** A-tier, not yet S-tier. Architecture, security fundamentals, and test discipline are strong, but three reproducible bugs block S-tier — one of them a data-loss bug: `ikk gc` is broken, `ikk check` false-positives on symlinked packages, and an unvalidated package name can make `ikk install`/`remove` delete the whole ikk home.

Gates: `cargo fmt --check` ✅ · `cargo clippy -D warnings` ✅ · `cargo test` = 64 core + 8 CLI + 1 real-world (2 GitHub e2e `#[ignore]`).

### High severity (reproduced)

1. **`ikk gc` always fails on the store lock file.** `ikk-cli/src/commands/gc.rs:22-40` iterates every store entry and `remove_dir_all`s anything not referenced by the lock — but the store dir contains `.lock` (held by `gc` itself via `Ctx::load`). `remove_dir_all` on a file errors, and the preceding chmod also flips `.lock` to `0o755`. Repro: `ikk gc` → `Error: Not a directory (os error 20)`; `ls ~/.ikk/store` shows `.lock` now `-rwxr-xr-x`. Fix: skip `.lock`/hidden files, or only delete entries containing `meta.toml` (the same predicate `find_all`/`verify_all` use).

2. **Symlinked packages falsely fail `ikk check`.** `store.rs::hash_dir` (line 302) hashes a symlink by its target string (`read_link`, line 314), but `copy_dir_contents` (line 327) dereferences symlinks into regular files. Stored tree ≠ hashed tree → `verify_all` reports `TAMPER DETECTED` on an unmodified install. Repro: local dir with `tool -> real-tool`, `ikk install` then `ikk check` → tamper alarm with two different hashes. Also: `copy_dir_contents` follows dir symlinks, so a `..`-cycle tarball recurses until failure. Fix: preserve symlinks on copy (and guard cycles).

3. **Unvalidated package name → `ikk install`/`remove` deletes the ikk home (data loss).** `link_bin` joins the raw name into `bin/<name>` and unconditionally `remove_dir_all`s whatever is there (`ops.rs:147,157`); `remove_dir_or_link` does the same for `ikk remove` (`ops.rs:226-234,259`). A name of `..` resolves `bin/..` to `~/.ikk` and deletes the entire home (config, lock, store, stage); `.` deletes `bin/` itself. Reproduced: `ikk install '..' --uri file://…` → `ikk.toml` and `store/` are gone before it errors "failed to create bin link". Same via `ikk remove '..'`, and `sync`'s `remove_stale` (any lock key named `..`). Fix: reject empty/`.`/`..`/path-separator names at CLI entry and defensively in `ops` before any path join.

### Medium severity

3. `upgrade --force` does not upgrade pinned packages — it un-skips the pin but `ops::install` still resolves the pinned version verbatim, so it re-downloads the same version and reports "already up to date" (`ikk-cli/src/commands/upgrade.rs:35,124`).
4. Bash rc mismatch on macOS — `Shell::rc_file()` returns `.bash_profile` (line 44) but `write_rc`/`remove_rc` hardcode `.bashrc` (lines 125/167); the PATH block lands in a file login shells never source, while `init` prints the `.bash_profile` path.
5. `RemoteSource::fetch` buffers the whole asset in memory with no progress (`source.rs:137-146`), unlike `UrlSource` which streams via `download_bytes`.
6. `ikk self-update` silently depends on `defaults.remote` — `self_update_repo` ("mandeepsmagh/ikk") is `owner/repo` shorthand, so `resolve_uri` needs `defaults.remote` to expand it. With no default remote set (`ikk init` allows skipping the prompt), `self-update` fails `Error: mandeepsmagh/ikk: relative URL without a base`. Repro: `ikk init --silent --no-shell` then `ikk self-update --check`. Fix: expand against a host derived from the repo value (or default to `github.com`) instead of `defaults.remote`.

### Low / hardening

- Dead code: `LockFile::diff` + `SyncPlan`, `Store::find_all` (no production callers).
- `check`'s "merkle root invalid" branch is unreachable — `LockFile::load` already verifies and errors first.
- Merkle root is unkeyed → detects accidental, not deliberate, tampering.
- `link_bin` spawns `cmd /C rmdir` on every platform (should be `#[cfg(windows)]`).
- `entry_name` slices `[..12]` guarded only by `debug_assert` (compiled out in release).
- `store.insert` leaves a partial entry (no `meta.toml`) if copy fails; `gc` would clean it, but `gc` is currently broken (finding 1).
- `install.sh` parses JSON with `grep|sed`, no `GITHUB_TOKEN`/prerelease filtering.

### Still deferred (unchanged)

- Live `ikk self-update` e2e (repo private → API 404).
- No `windows-arm64` / `linux-musl` release assets.
- Length-prefix lockfile Merkle leaves + `hash_dir` field separators.

---

## Follow-up Review (2026-08-21) — closed

**Verdict:** S-tier. The two CLI logic bugs and the dry-run honesty gap found in the prior review are fixed; input-dependent `.unwrap()`/`.expect()` calls removed; CLI now has test coverage. All gates green (62 core + 8 CLI tests).

Fixed:

1. ✅ `config set defaults.self_update_repo` validation was a no-op (`!count == 1` → always false) — now requires exactly one `/`.
2. ✅ `ikk upgrade` skipped packages with no `version` field — now only concrete non-`latest` pins are skipped.
3. ✅ `sync --dry-run` now applies the release age/quality gate (shared `gate_release`), matching a real sync.
4. ✅ Removed input-dependent `.unwrap()`/`.expect()`: `attach_dmg` path, `ConfigRegistry::new` (now fallible), `single_executable`.
5. ✅ Latent bugs: `find_all` dash-name cross-match, `truncate_label` UTF-8 slice panic, non-atomic `ikk.toml` save.
6. ✅ 8 CLI tests added (`config`, `upgrade`) + core tests for `gate_release`, `find_all`, `truncate_label`.

### Deferred (unchanged)

- Live `ikk self-update` e2e — repo `mandeepsmagh/ikk` is private (GitHub API 404).
- No `windows-arm64` / `linux-musl` release assets.
- Optional: length-prefix lockfile Merkle leaves + `hash_dir`; progress bar on forge downloads.

---

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
