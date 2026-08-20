# REVIEW — Release Pipeline & Remaining Gaps

**Date:** after v0.8.0 release run (green: 5 binaries + `SHA256SUMS` uploaded).
**Scope:** `.github/workflows/release.yml`, `install.sh`/`install.ps1`, self-update/asset scoring, versioning.

## 🔴 Critical

### 1. `score_asset` does not recognize `x86_64`
`ikk-core/src/platform.rs`:

```rust
let tokens: Vec<&str> = name.split(['-', '_', '.']).collect();
```

This splits `x86_64` into `x86` + `64`, but `Arch::X86_64::variants()` contains the literal
`"x86_64"`. Result: any asset named `*-x86_64-*` (e.g. `ikk-linux-x86_64.tar.gz`) never gets an
arch+os match — it only matches through the os-only fallback, and can **tie** with the aarch64
asset of the same OS.

`processor.rs` and `self_update.rs` both select with `max_by_key`; ties resolve to the *last*
equally-scored asset. Correctness currently depends on GitHub API asset ordering (luck).

**Fix:** in the `contains` closure, also match raw-name substrings for variants containing `_`
(specifically `x86_64`), or otherwise keep `x86_64` intact during tokenization. Add a regression
test that asserts the correct arch asset *wins* (higher score), not just `.is_some()`.

### 2. `install.sh` / `install.ps1` are broken against the new release
The release publishes `ikk-{os}-{arch}.{ext}` + `SHA256SUMS` only. The install scripts still:

- build **old target-triple** URLs:
  - `install.sh` → `ikk-x86_64-unknown-linux-gnu.tar.gz`, `ikk-aarch64-apple-darwin.tar.gz`
  - `install.ps1` → `ikk-x86_64-pc-windows-msvc.zip`
- download `{url}.sha256`, but **per-asset `.sha256` sidecars are no longer published** → 404.
- `install.sh` then runs `sha256sum -c` against a sidecar whose embedded filename doesn't match
  the local download name anyway.

`curl …/install.sh | sh` and `irm …/install.ps1 | iex` are currently dead paths.

**Fix:** rewrite both scripts to (a) use `ikk-{os}-{arch}.{ext}` naming and (b) verify against the
published `SHA256SUMS` (mirroring `self_update.rs`), or publish per-asset `.sha256` sidecars again.

### 3. Version mismatch: crates `0.7.1` vs tag `v0.8.0`
`ikk-cli/Cargo.toml` and `ikk-core/Cargo.toml` are both `0.7.1`; the release/tag is `v0.8.0`.
The published binary reports `0.7.1` while `releases/latest` resolves to `v0.8.0`.

**Fix:** bump both crates to `0.8.0`, refresh `Cargo.lock`, re-tag `v0.8.0` (or tag `v0.7.2`).

## 🟠 High

### 4. Checksum verification is inconsistent
- `self_update.rs` verifies via `SHA256SUMS` ✅ (published).
- Install scripts expect per-asset `.sha256` ❌ (not published).

Prefer: install scripts verify against `SHA256SUMS`, keeping a single published checksum file.

### 5. §4 not closed
Self-update end-to-end gate still pending: `ikk self-update --check` → `ikk self-update`, asset
matched, checksum verified, **no** "skipping verification" note. Blocked by #1 and #3.

## 🟡 Medium / Low

6. **README** install docs route through the broken scripts; update after #2.
7. **Minor robustness:** non-test `.unwrap()`/`.expect()`:
   - `processor.rs` `attach_dmg`: `dmg_path.to_str().unwrap()`
   - `registry.rs`: `.expect("built-in remotes.toml …")`
   Low risk; built-in data is trusted, but the path `to_str()` can panic on non-UTF8 paths.
8. **Platform coverage:** no `windows-arm64` or `linux-musl` release assets. `self-update` will
   report "no ikk release asset" on those platforms.

## Verified clean (this review)

- `cargo fmt --all -- --check` ✅
- `cargo clippy --all-targets -- -D warnings` ✅
- `cargo test` ✅ (56 passed; 2 GitHub e2e tests `#[ignore]`d by design)
