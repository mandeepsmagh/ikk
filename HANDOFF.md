# HANDOFF — S-Tier Package Management Refactor

**Branch:** `refac/core-arch` · **Last commit:** `3412e49` · **State: WIP — core done, CLI broken**

## The model (read this first)

Everything is a **directory**. There is no single-binary concept anywhere.

```
fetch → Artifact { dir, archive_hash, source_url }
      → store.insert(artifact)   → ~/.ikk/store/<hash12>-<name>-<version>/
      → link_bin()               → ~/.ikk/bin/<name>/  (symlink/junction → store entry)
      → lock.insert(name, LockedPackage)
```

- `bin/<name>/` is a **per-package directory link** to the store entry. Each package
  owns its own subdirectory, so author-native binary names (nvim, rg, foo.exe) can
  never collide. No aliasing, no `binary` field, no collision resolution.
- Shell PATH exports `bin/` **and every `bin/*/` subdir** (`shell.rs::path_exports`).
- The lockfile `tree_root` is a sorted-leaf Merkle digest over
  `name+version+uri+sha256+bin_entry+variant`.
- Store integrity is `hash_dir` over the whole entry; `seal()`/`unseal()` are gone.
  Tampering is *detected* by `verify_all()`, not prevented by permissions.

## What's done (ikk-core — compiles, 52/53 unit tests pass)

| File | Change |
|------|--------|
| `source.rs` | `Artifact` struct; `Source` trait (`version` + `fetch`); `RemoteSource` (GitHub releases), `UrlSource`, `LocalSource` (with optional `[build]` steps). All fetch paths end in `extract::extract_dir`. |
| `extract.rs` | `extract_dir` only (tar.gz / zip / raw-dir copy). `unwrap_single_root` descends into a lone top-level dir (e.g. `nvim-linux-x86_64/`). `best_asset` still picks the platform-appropriate release asset. |
| `store.rs` | Single `insert(name, version, variant, &Artifact)`. Entry name = `{hash12}-{name}-{version}`. `hash_dir` integrity. No seal. |
| `ops.rs` | One pipeline: `install_from_source` (resolve → fetch → store → link → lock). `install` / `install_template` / `install_local` are thin wrappers choosing the `Source`. `remove(name, ...)` takes **no binary param**. `link_bin` creates the `bin/<name>/` junction with a `cmd /C rmdir /S /Q` fallback for Windows. |
| `shell.rs` | PATH = `bin/` + each `bin/*/`. |
| `config.rs` | `PackageConfig` has **no `binary` field**. |
| `lock.rs` | `LockedPackage` has **no `binary` field**; `is_dir` kept only for old-lock deserialization (always true now). |
| `error.rs` | `BuildBinaryNotFound` and `BinaryNotFound` removed. |

## What's broken (do these in order)

1. **`ikk-cli` does not compile.** It still uses the old API:
   - `commands/remove.rs` — looks up `pkg.binary` and calls `ops::remove(name, binary, ...)`. New signature: `ops::remove(name, &home, &store, &mut lock)`.
   - `commands/run.rs` — branches on `locked.is_dir` / single-binary. Now: run from `bin/<name>/` (find the executable inside, or treat `name` as the binary name inside that dir).
   - `commands/self_update.rs` — reads/writes `binary` field.
   - `commands/sync.rs`, `commands/upgrade.rs` — call old `ops::remove` signature.
   - `commands/add.rs` — has a `--binary` flag; remove it.
2. **Integration tests** (`ikk-core/tests/github_e2e.rs`, `real_world.rs`) reference
   removed APIs (`binary` field, single-binary `extract`). Update or delete.
3. **One flaky Windows test:** `ops::tests::remove_unlinks_and_cleans` fails because
   Windows briefly locks a junction after creation; `remove_file` gets os error 5.
   Fix: give `remove()` the same `cmd /C rmdir /S /Q` fallback that `link_bin` has
   (see `link_bin` in `ops.rs`), or add a short retry.
4. Run `cargo test` (full) and `cargo clippy --workspace` before committing the CLI.

## Conventions

- Rust 2024, `cargo fmt` (4-space, no mixed), clippy pedantic with the crate-level
  allows in `lib.rs`.
- Errors: `thiserror` in `error.rs`; `tracing` for logs.
- Tests use `tempfile::tempdir()`; home layout via `IkkHome::new(path)`.
