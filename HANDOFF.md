# HANDOFF — ikk

**Last session:** 2026-08-22 — full S-tier re-review. No code changes; findings recorded in `REVIEW.md` (§ "S-tier Review (2026-08-22)").

## State

- Re-reviewed all of `ikk-core` + `ikk-cli`. Gates still green: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (64 core + 8 CLI + 1 real-world).
- Verdict: **A-tier, not yet S-tier** — three reproducible high-severity bugs, all confirmed live (one is data loss):
  1. **`ikk gc` broken** — iterates the store dir and `remove_dir_all`s `.lock` (which `gc` itself holds via `Ctx::load`). Fails `Error: Not a directory (os error 20)` and chmods `.lock` to 0o755. (`ikk-cli/src/commands/gc.rs:22-40`)
  2. **Symlinked packages falsely fail `ikk check`** — `hash_dir` hashes symlink *targets* (`store.rs:314`) but `copy_dir_contents` *dereferences* them (`store.rs:327`), so the stored tree re-hashes differently → false `TAMPER DETECTED` on an unmodified install. Dir-symlink cycles also recurse unboundedly. Repro: local dir with `tool -> real-tool`, install, `ikk check`.
  3. **Unvalidated package name → data loss** — `link_bin`/`remove_dir_or_link` join the raw name into `bin/<name>` and `remove_dir_all` it (`ops.rs:147,157,234,259`). Name `..` → `bin/..` = `~/.ikk` deleted (config, lock, store gone); `.` deletes `bin/`. Reproduced. Same via `ikk remove '..'` and `sync` remove_stale.
- 4 medium + hardening list (dead code, unkeyed Merkle root, `--force` no-op, bash rc mismatch, unbuffered forge downloads, self-update depends on `defaults.remote`) — all in REVIEW.md.

## Next session

- Fix the three high-severity bugs first (same commit as this handoff's successors):
  - `gc`: skip `.lock`/hidden entries, or only remove entries containing `meta.toml`.
  - `copy_dir_contents`: re-create symlinks instead of dereferencing (store tree must match `hash_dir`); add a cycle guard.
  - Package-name validation: reject empty/`.`/`..`/path-separator names at CLI entry (and defensively in `ops::link_bin`/`remove`).
- Add regression tests: `gc` skips `.lock`; symlink-preserving copy + `ikk check` passes on a symlinked package; `install`/`remove` reject `..`/`.` without touching `~/.ikk`.
- Then the mediums (optional, in priority order): 6 self-update vs `defaults.remote` coupling, 5 stream forge downloads via `download_bytes`, 3 `upgrade --force` semantics, 4 bash rc path.

## Broken boundaries / known flakes

- None in the gate suite itself. The two bugs above are the only known breakage; exact repros and fix pointers live in `REVIEW.md`.
