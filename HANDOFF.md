# HANDOFF — S-Tier Package Management Refactor

**Branch:** `refac/core-arch` · **State: core refactor done; CLI smoke pass incomplete; 1 unexplained anomaly blocks verification**

## The model (read this first)

Everything is a **directory**. There is no single-binary concept anywhere.

```
fetch → Artifact { dir, archive_hash, source_url }
      → store.insert(artifact)   → ~/.ikk/store/<hash12>-<name>-<version>/
      → link_bin()               → ~/.ikk/bin/<name>/  (symlink/junction → store entry)
      → lock.insert(name, LockedPackage)
```

- `bin/<name>/` is a **per-package directory link** to the store entry. Each package
  owns its own subdirectory, so author-native binary names can never collide.
- Shell PATH exports `bin/` **and every `bin/*/` subdir** (`shell.rs::path_exports`).
- Lockfile `tree_root` is a sorted-leaf Merkle digest over
  `name+version+uri+sha256+bin_entry+variant`.
- Store integrity is `hash_dir` over the whole entry; tampering detected by
  `verify_all()`, not prevented by permissions.

## What's done (all verified green)

- `ikk-core`: unified `Artifact` pipeline, `extract_dir` only, single
  `store.insert(name, version, variant, &Artifact)` with `hash_dir` integrity,
  one `install_from_source` pipeline in `ops.rs`, per-package `bin/<name>/` links,
  Merkle lockfile. **No `binary` field anywhere.**
- `ikk-cli`: fully migrated to the new pipeline; `--binary` flag removed from `add`.
- Integration tests updated (`github_e2e.rs`, `real_world.rs`).
- **All green:** `cargo test --workspace` (55 core + real_world pass; e2e ignored by
  design), `cargo clippy --workspace` 0 warnings, `cargo fmt` clean.

### Fixes landed in `d821edc` (this session)

1. **Local install bin-link crash (handoff bug #1 — FIXED).**
   Root cause: the CLI flow never creates `~/.ikk/bin/` (only `ikk init` did), so
   `symlink(target, bin/<name>)` in `link_bin()` died with a bare ENOENT. Fix:
   `create_dir_all(bin_dir)` at the top of `link_bin()` (`ikk-core/src/ops.rs`) plus
   a named `IkkError::Store` with the link path on symlink failure. Verified via the
   real CLI: `IKK_HOME=$T/.ikk ikk install mytool --uri $T/pkg` now succeeds and
   creates `bin/mytool → store/<hash>-mytool-local/bin`.

2. **Config round-trip (`ikk-core/src/config.rs`).**
   `add.rs` saves packages under `[packages.<name>]`, but `Config::load` parsed every
   top-level section as a package entry — so the next command hit "missing field
   uri". Fix: added `"packages"` to `KNOWN_SECTIONS`.

3. **`ikk run` default binary (`ikk-cli/src/commands/run.rs`).**
   Restored the lost fallback: when no binary is named, try the package name, else
   the sole executable in the package; ambiguous case lists available binaries.

## ⚠️ Unresolved anomaly — verify first thing next session

**Symptom:** after `ikk install mytool --uri <local-dir>`, `ikk list` prints
"no packages configured" and `ikk info mytool` says "not found in config", even
though `$IKK_HOME/ikk.toml` on disk contains `[packages.mytool] uri = "..."`.

**What's been ruled out (do NOT re-verify these):**
- Source is correct: `KNOWN_SECTIONS` includes `"packages"`; the load loop at
  `config.rs:235-248` skips known sections and returns `packages` in `Self { .. }`
  (verified byte-level with `od -c`).
- TOML structure is as expected: a standalone rustc+toml-1.1.2 program parsing the
  exact saved file yields top keys `auth, defaults, packages, remotes, security, store`.
- Single toml crate in the graph (v1.1.2); no cargo config overrides; fresh rebuilds
  used; unit tests pass (`deserialize_top_level_packages` uses **top-level**
  `[ripgrep]` sections — it does NOT cover the nested `[packages.x]` shape that
  `save()` writes).

**Next steps (in order):**
1. Add a temporary `eprintln!` inside `Config::load` printing each top-level key and
   the final `packages.len()`, rebuild, run the repro below, read the output. This
   distinguishes "load returns empty" from "list reads a different file".
2. Repro:
   ```
   T=$(mktemp -d); mkdir -p $T/pkg/bin; printf '#!/bin/sh\necho hi\n' > $T/pkg/bin/hello; chmod +x $T/pkg/bin/hello
   IKK_HOME=$T/.ikk ./target/debug/ikk install mytool --uri "$T/pkg"
   cat $T/.ikk/ikk.toml      # shows [packages.mytool]
   IKK_HOME=$T/.ikk ./target/debug/ikk list   # currently: "no packages configured"
   ```
3. Suspects not yet checked: whether `list`/`info` read a different path than
   `add` writes (both use `home.config_file()` per grep, but confirm at runtime),
   or a stale build artifact being executed.

## What's left (after the anomaly)

1. **Finish CLI smoke pass** — verified so far: install (local dir), run (named +
   ambiguous-default error). Still to exercise: check, sync, upgrade, gc, remove,
   init (PATH block in rc file), and list/info once the anomaly is resolved.
2. **Known flaky Windows test:** `ops::tests::remove_unlinks_and_cleans` — Windows
   briefly locks a junction after creation; `remove_file` gets os error 5. Fix: give
   `remove()` the same `cmd /C rmdir /S /Q` fallback that `link_bin` has, or a short
   retry. (Not reproducible on macOS.)
3. When done, **delete `HANDOFF.md`** — permanent record is `ikk-core/ROADMAP.md` +
   git history.

## Conventions

- Rust 2024, `cargo fmt` (4-space), clippy pedantic with the crate-level allows in
  `lib.rs`.
- Errors: `thiserror` in `error.rs`; `tracing` for logs.
- Tests use `tempfile::tempdir()`; home layout via `IkkHome::new(path)`.
