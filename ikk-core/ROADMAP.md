# ikk-core: Roadmap to S-Tier Package Management

This document outlines the strategic architectural shifts required to transform `ikk-core` from a functional utility into a world-class, high-performance, and production-ready package management engine.

## Status (branch `main`)

| Item | State |
|------|-------|
| §1.A Remove `binary_name` from core | ✅ done — no `binary` field in core or CLI |
| §1.B Unified install pipeline | ✅ `install_from_source` is the single path; wrappers are thin |
| §1.C Unified storage (`store.insert(artifact)`) | ✅ `insert`/`insert_dir` merged; seal/unseal removed |
| §1.D Pure fetching | ✅ `Source::fetch` returns `RawContent`; `RawContent::process` (processor stage) does detection + extraction |
| §2 Integrity auditing over sealing | ✅ `hash_dir` + `verify_all`; no permission-based sealing |
| §3 Flat-dir model, per-package `bin/<name>/` links | ✅ core done (junction/symlink + copy fallback) · CLI `run`/`remove` migrated |
| ikk-cli migration | ✅ done — config round-trip fixed, CLI smoke pass complete (install/list/info/check/sync/upgrade/gc/remove/init) |
| Integration tests | ✅ updated to new APIs |
| §4 Release asset naming + SHA256SUMS | ✅ done — asset naming, `SHA256SUMS`, `score_asset` x86_64, 0.8.0 bump, install scripts; live `self-update` e2e closed (repo public, unauthenticated pass, `v`-prefix fix) |
| §5 S-tier review gaps | ✅ all 9 fixed — `AuthConfig` deleted, fail-closed self-update checksum (+`--insecure`), honest `sync --dry-run`, `upgrade` failure summary, `gc` store lock, UA from `CARGO_PKG_VERSION`, mode-bit exec check, `config get/set defaults.self_update_repo`, `install.ps1` on `curl.exe` |
| Follow-up review (2026-08-21) | ✅ done — self-update-repo validation no-op fixed, `upgrade` skipped `None`-version pkgs fixed, dry-run age gate shared via `gate_release`, input-dependent `.unwrap()`/`.expect()` removed (`registry`, `attach_dmg`, `run`), `find_all`/`truncate_label` latent bugs fixed, atomic `ikk.toml` save, 8 CLI tests added |
| S-tier re-review fixes (2026-08-22) | ✅ done — `gc` skips `.lock`/meta-less entries, symlink-preserving `copy_dir_contents`, package-name validation (data-loss closed), `upgrade --force` resolves `latest`, bash rc path unified, forge downloads streamed, `self-update` `github.com` fallback |
| Live self-update e2e (2026-08-22) | ✅ done — repo public; unauthenticated `--check` + asset/`SHA256SUMS` verified; fixed `v`-prefix version comparison |
| macOS static `liblzma` (2026-08-22) | ✅ done — `xz2` `static` feature; macOS assets self-contained (no Homebrew dylib); `v0.8.3` release required to ship |
| windows-arm64 asset (2026-08-22) | ✅ done — `release.yml` `aarch64-pc-windows-msvc` matrix row, `install.ps1` `ProcessArchitecture` detection, `score_asset` native-beats-emulated test; ships with next tag |
| Flat `bin/` executable links (2026-08-23) | ✅ done — `~/.ikk/bin` is one symlink per executable (recursive scan, collision-rejecting), `ikk.lock` records `bins`, `ikk run`/`ikk check` resolve via store entry; `self_update_repo` serde-defaulted + backfilled by `ikk init` |
| Content-based executable classifier (2026-08-24) | ✅ done — `binary::is_runnable` classifies by file content (shebang / Mach-O `MH_EXECUTE` / ELF `ET_EXEC` + `ET_DYN`+`PT_INTERP` / Windows extension), shared by `ops` + `ikk run`; stops `.dylib`/`.so` leaking into `~/.ikk/bin` (llama.cpp `bin/`, neovim `lib/`) |
| Classifier v2 rewrite (2026-08-26) | ✅ done — `binary.rs` is now allocation-free `classify(bytes, path)` → `Classification{Format,Role,Architecture}`; cross-host ELF/Mach-O/PE parsing, bounded metadata views + checked arithmetic; `is_runnable` kept as compat wrapper. Classification *persistence* deferred (see REVIEW.md) — in-memory pass-through at `store::insert` is the only worthwhile optimization; CAS bytes + `content_sha256` make a disk cache redundant |
| Symlink containment (2026-08-24) | ✅ done — `ops::is_within_root` rejects executables whose canonical target escapes the store root, at PATH-export (`link_executables`) and `ikk run` (`find_binary`/`single_executable`/`list_binaries`); 5-case symlink matrix test |
| ZIP path containment (2026-08-24) | ✅ done — `processor.rs` uses `ZipFile::enclosed_name()` (Windows-aware) + `starts_with(out_dir)` assert; `safe_join` removed; 5-case traversal matrix + nested-layout regression tests |

---

## 1. Architectural Simplification (The "Minimalist Engine" Principle)

An S-Tier core must be a "dumb" but extremely fast engine. Currently, too much "decision making" is happening in the orchestration layer.

### A. Decouple Identity from Implementation (`ops.rs`)
**Problem:** The core currently struggles with a "dual identity" crisis. It tries to manage both the **Package Identity** (the name/version) and the **Physical Identity** (the specific binary name/path). This leads to complex parameter passing (e.g., `binary_name`) and potential conflicts between `pkg.binary` and what is actually found in an archive.

**Recommendation:**
* **Complete Removal of `binary_name` from the Core:** The orchestration layer (`ops.rs`) should not know, pass, or care about the name of the executable. It should only care about the `Package` and the `Source`.
* **Shift Responsibility to the Artifact:** The knowledge of "what the binary is called" should be a property of the `Artifact` (the result of a fetch) or the `Source`. The `Source` discovers the name; the `Store` records it; `Ops` simply moves the data.
* **Result:** `ops.rs` becomes a pure, "blind" workflow: `Source` $\rightarrow$ `Artifact` $\rightarrow$ `Store`. This eliminates "name mismatch" bugs and simplifies all function signatures.

### B. Unify the Orchestration Pipeline (`ops.rs`)
**Problem:** `ops.rs` currently contains multiple installation paths (`install`, `install_template`, `install_local`), leading to branching logic based on the "mode" of the package.

**Recommendation:**
* **Single Installation Path:** Collapse all installation methods into a single `install(Source)` function.
* **Source-Driven Logic:** The CLI should determine the source type (Remote, URL, or Local) and pass a unified `Box<dyn Source>` to the core.
* **Result:** `ops.rs` shrinks from a collection of "how-to" recipes into a single, high-speed, linear pipeline.

### C. Unify the Storage Model (`store.rs`)
**Problem:** The `Store` currently has two distinct code paths: `insert` (for single files) and `insert_dir` (for multi-binary archives). This forces the caller (`ops.rs`) to implement branching logic.

**Recommendation:**
* **Unified Content-Addressing:** Treat everything as a "Content-Addressed Entry." Whether it is one binary or a 10GB directory, the Store should simply accept an `Artifact` and map it to a hash-based directory.
* **Remove Branching:** Eliminate the `if fetched.is_dir` checks in `ops.rs`. The Store's API should be: `store.insert(artifact)`.

### D. Pure Fetching (`source.rs`) — ✅ done
**Problem:** `source.rs` is currently "heavy" because it handles extraction and archive detection. This makes adding new source types (like Git or S3) difficult because they must also implement extraction logic.

**Recommendation:**
* **The "Raw Fetch" Pattern:** A `Source` should only be responsible for fetching raw bytes. 
* **Introduce an `ArtifactProcessor`:** Move the logic for "detecting archive type $\rightarrow$ extracting $\rightarrow$ picking best binary" into a dedicated lifecycle stage. 
* **Result:** `source.rs` becomes a lightweight interface that is trivial to extend.

**Implementation:** `Source::fetch` returns `RawContent` (`Bytes { bytes, filename }` or `Directory { path }`). The processor stage is `RawContent::process(stage_dir) -> Artifact` in `processor.rs`, which owns `ArchiveKind` detection, `extract_dir`, and hashing. `ops.rs` pipeline: `source.fetch(...)` → `raw.process(&stage)` → `store.insert(artifact)`.

---

## 2. Security Evolution (The "Real-World Usability" Principle)

**Problem:** The current security model is "brittle." Specifically, the "Sealing" mechanism (making files read-only via `0o555`) is a major friction point. In real-world environments (like Neovim or complex build systems), binaries often need to be modified, patched, or interact with their own metadata. A hard `chmod` makes the tool feel "hostile" to the user's environment.

### A. Move from "Hard Sealing" to "Integrity Auditing"
**Problem:** `seal()` (setting `0o555`) is a destructive/permanent state change that causes runtime errors when software expects write access.

**Recommendation:**
* **Remove `seal()`/`unseal()`:** Stop using filesystem permissions as a security mechanism. It is too blunt an instrument.
* **Adopt "Continuous Verification":** Instead of making files read-only to prevent tampering, use the `Store::verify_all()` and `LockFile::verify()` mechanisms to **detect** tampering. 
* **The S-Tier approach:** Allow the filesystem to behave naturally, but treat any mismatch between the `Store` metadata and the actual file hash as a **Critical Security Event** that halts execution.

### B. Strengthen the Integrity Chain
* **Merkle-Tree Lockfiles:** Continue the excellent work on the `tree_root` in the lockfile, but ensure it is the **single source of truth** for the entire environment.
* **Hardware-Rooted Integrity (Future):** Design the core so that the `LockFile` verification could eventually be offloaded to a TPM or a signed remote manifest.

---

## 4. Release Pipeline & Self-Update Trust (Next Session)

**Problem:** `release.yml` packages assets as `ikk-{cargo-target-triple}.tar.gz` (e.g. `ikk-x86_64-unknown-linux-gnu.tar.gz`), but `self_update.rs` picks the platform asset via `score_asset()` — which expects `tool-{os}-{arch}` style names. Target-triple names score 0, so self-update fails with "no ikk release asset for platform". Additionally, `self_update` verifies against a published `SHA256SUMS` file, but the release only publishes per-asset `.sha256` sidecars (and the Windows `certutil` one is multi-line with a header, not `hash  filename` format) — so verification is silently skipped every time.

**Recommendation:**
* **Rename assets to `ikk-{os}-{arch}.{ext}`** — e.g. `ikk-linux-x86_64.tar.gz`, `ikk-darwin-aarch64.tar.gz`, `ikk-darwin-x86_64.tar.gz`, `ikk-windows-x86_64.zip`. Map the cargo target triple in `release.yml` (a small lookup table in the packaging step). This matches `score_asset()` conventions and is the name format the owner prefers.
* **Publish one `SHA256SUMS`** in the release job: after downloading all artifacts, write `sha256sum`-format lines (`<hash>  <name>`) and upload it as a release asset. `self_update.rs` already parses exactly this format (`name == asset_name || name == *{asset_name}`).
* **Verify with a real test:** after the branch merges to main and the next tag is cut, run `ikk self-update --check` / `ikk self-update` against the published release and confirm asset match + checksum verification (no "skipping verification" note).

**Status:** release.yml rewritten and verified green (per-asset `.sha256` sidecars; `download-artifact@v8` `pattern: ikk-*` + `merge-multiple: true`; release job concatenates `artifacts/*.sha256` into `SHA256SUMS`). v0.8.0 uploaded 5 binaries + non-empty `SHA256SUMS`. Remaining gaps (score_asset x86_64 matching, install scripts, version mismatch, self-update test) are tracked in `REVIEW.md`.
* **Optional (later):** sign the release (GPG/sigstore) so `SHA256SUMS` itself is trusted, not just present.

## 3. Performance & UX (The "Zero Friction" Principle)

### A. Zero-Copy/Zero-Move Operations
* **Symlink-First Strategy:** Ensure the `bin` directory always uses symlinks (or junction points on Windows) to point into the `Store`. This makes "installations" nearly instantaneous and allows for atomic "swaps" of package versions.

### B. Predictable Path Integration
* **Idempotent Shell Integration:** The current `shell.rs` is good, but ensure that path integration is "invisible." The user should never feel like they are "configuring" ikk; it should just work.

---

## Summary of the S-Tier State

| Feature | Current State | S-Tier State |
| :--- | :--- | :--- |
| **Logic Flow** | Branching, complex parameter passing | Linear, single-path pipeline |
| **Security** | Brittle (chmod 0555) | Robust (Continuous Hash Auditing) |
| **Extensibility** | Difficult (Extraction tied to Source) | Trivial (Source fetches, Processor handles) |
| **Storage** | File vs. Directory distinction | Unified Content-Addressed Entries |
| **Reliability** | Detection of tampering | Prevention of corruption + Detection of tampering |
