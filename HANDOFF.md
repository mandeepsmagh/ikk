# HANDOFF — ikk

**Last session:** 2026-08-22 — fixed all S-tier re-review findings (3 high + 4 medium + cleanups). Gates green: fmt, clippy (`-D warnings`), `cargo test` (66 core + 9 CLI + 1 real-world).

## State

- **`ikk gc`** no longer trips on the store `.lock` — collects only `meta.toml`-bearing directories (`gc.rs::is_store_entry`).
- **Symlink integrity** — `store::copy_dir_contents` re-creates symlinks (matches `hash_dir`) instead of dereferencing; cycle-safe, Windows copy fallback; `processor` DMG path reuses it.
- **Package-name validation** — `ops::validate_name` rejects `.`/`..`/separators/metacharacters; enforced in `link_bin`/`remove` + `add`/`remove`/`run`. The `ikk install '..'` data-loss path is closed.
- **`upgrade --force`** now drops the pin and resolves `latest`.
- **Bash rc** — `write_rc`/`remove_rc`/`rc_file` share `Shell::bash_rc_file` (macOS `.bash_profile` convention).
- **Forge downloads** stream via `progress::download_bytes` (bearer-aware, progress bar).
- **`self-update`** expands `self_update_repo` with a `github.com` fallback when `defaults.remote` is unset.
- Cleanups: removed `LockFile::diff`/`SyncPlan` + `Store::find_all`; simplified dead `check` branch; case-insensitive `sha256` pin; safe `entry_name` slice; `#[cfg(windows)]` cmd sweep; `store.insert` cleans partial entries on failure.

## Next session

- Live `ikk self-update` e2e against the private repo **with `GITHUB_TOKEN` set** (asset match + SHA256SUMS verification) — still the last un-run gate.
- Optional (documented, not bugs): unkeyed Merkle root hardening (length-prefix leaves / signing), `install.sh` JSON parsing via a real parser, `windows-arm64`/`linux-musl` release assets.

## Broken boundaries / known flakes

- None. All gates green; the three high-severity repros from the prior session are covered by regression tests.
