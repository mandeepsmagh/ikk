# REVIEW — Release Pipeline (closed)

**Date:** after v0.8.0 re-tag (`65303f3`).
**Status:** all code items resolved. Live end-to-end `ikk self-update` verification deferred — the repo is private, so `api.github.com/.../releases/latest` returns 404 without a token. Re-run the §4 gate once the repo is public (or with an authenticated API client).

## Resolved

1. **`score_asset` x86_64** — `contains` closure now matches separator-containing variants as raw substrings; regression test `score_x86_64_beats_wrong_arch` asserts the matching-arch asset wins.
2. **`install.sh` / `install.ps1`** — rewritten for `ikk-{os}-{arch}.{ext}` naming; verify against published `SHA256SUMS`.
3. **Version mismatch** — crates bumped `0.7.1` → `0.8.0`; tag re-pointed at `65303f3`.
4. **Checksum consistency** — single `SHA256SUMS` file, used by self-update and both install scripts.
5. **§4 gate** — code complete; live run blocked by private-repo 404 (see top).
6. **README** — routes through the fixed scripts; no change needed.

## Deferred (low risk)

- **Minor robustness:** non-test `.unwrap()`/`.expect()` in `processor.rs` `attach_dmg` (`to_str()`) and `registry.rs` (built-in `remotes.toml`). Trusted built-in data.
- **Platform coverage:** no `windows-arm64` / `linux-musl` release assets; self-update reports "no ikk release asset" there.
