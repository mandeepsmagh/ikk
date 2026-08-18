# HANDOFF — S-Tier Package Management Refactor

**Branch:** `refac/core-arch` · **State: nearly done — 1 CLI runtime bug left**

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

## What's done (all verified green)

- `ikk-core`: unified `Artifact` pipeline (`source.rs`), `extract_dir` only
  (`extract.rs`), single `store.insert(name, version, variant, &Artifact)` with
  `hash_dir` integrity and no seal, one `install_from_source` pipeline in `ops.rs`,
  per-package `bin/<name>/` links, Merkle lockfile. **No `binary` field anywhere.**
- `ikk-cli`: fully migrated — `--binary` flag removed from `add`, `remove` uses the
  new 4-arg `ops::remove`, `run` discovers executables inside `bin/<name>/`,
  `sync`/`upgrade` updated, `init` uses new `shell::write_rc`, `self_update` clean.
- Integration tests updated (`github_e2e.rs`, `real_world.rs`).
- **All green:** `cargo test --workspace` (55 core + real_world pass; e2e ignored by
  design), `cargo clippy --workspace` 0 warnings, `cargo fmt` clean.
- Bug fixed during smoke testing: stage cleanup in `install_from_source` destroyed
  local sources nested under `$IKK_HOME`; now guarded with `stage.exists()`.

## What's left (do these in order)

1. **CLI runtime bug — local install fails at the bin-link step.**
   `ikk install mytool --uri <local-dir>` prints `stored mytool@local (<hash>)` then
   dies with a bare `io::Error` "No such file or directory" (no context). Evidence:
   - Store entry is created correctly (`store/<hash12>-mytool-local/bin/hello`).
   - `$IKK_HOME/bin/` does **not** exist afterwards — the link creation in
     `link_bin()` (`ikk-core/src/ops.rs`) fails before creating anything.
   - The identical pipeline passes in the unit test
     `ops::tests::install_local_directory_end_to_end` (source nested under home), so
     the difference is something specific to the CLI path — suspect how
     `Ctx::load` (`ikk-cli/src/commands/mod.rs`) resolves store/home paths, or a
     missing dir in the CLI flow that unit-test `setup()` creates.
   - **Next step:** add an `eprintln!`/tracing with the link path + error in
     `link_bin()`, run once:
     ```
     T=$(mktemp -d); mkdir -p $T/pkg/bin; printf '#!/bin/sh\necho hi\n' > $T/pkg/bin/hello; chmod +x $T/pkg/bin/hello
     IKK_HOME=$T/.ikk ./target/debug/ikk install mytool --uri "$T/pkg"
     ```
     Or write a minimal repro test that calls `Ctx::load` + `ops::install_local`
     exactly like `add.rs` does.
   - Also add context to the error (e.g. map io errors in `link_bin` to a named
     `IkkError` variant) so future failures are diagnosable.

2. **Known flaky Windows test:** `ops::tests::remove_unlinks_and_cleans` — Windows
   briefly locks a junction after creation; `remove_file` gets os error 5. Fix: give
   `remove()` the same `cmd /C rmdir /S /Q` fallback that `link_bin` has, or a short
   retry. (Not reproducible on macOS.)

3. **Full CLI smoke pass** once #1 is fixed — exercise each command at least once:
   install (local dir + local archive), run (named + default binary), list, info,
   check, remove, sync, upgrade, gc, init (PATH block in rc file).

4. When the handoff work is complete, **delete `HANDOFF.md`** — the permanent record
   is `ikk-core/ROADMAP.md` + git history.

## Conventions

- Rust 2024, `cargo fmt` (4-space), clippy pedantic with the crate-level allows in
  `lib.rs`.
- Errors: `thiserror` in `error.rs`; `tracing` for logs.
- Tests use `tempfile::tempdir()`; home layout via `IkkHome::new(path)`.
