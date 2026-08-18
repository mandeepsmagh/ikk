# ikk-core: Roadmap to S-Tier Package Management

This document outlines the strategic architectural shifts required to transform `ikk-core` from a functional utility into a world-class, high-performance, and production-ready package management engine.

## Status (branch `refac/core-arch`)

| Item | State |
|------|-------|
| §1.A Remove `binary_name` from core | ✅ done — no `binary` field in core or CLI |
| §1.B Unified install pipeline | ✅ `install_from_source` is the single path; wrappers are thin |
| §1.C Unified storage (`store.insert(artifact)`) | ✅ `insert`/`insert_dir` merged; seal/unseal removed |
| §1.D Pure fetching | ⚠️ partial — extraction lives in `extract.rs` (called by `Source::fetch`), not a separate processor stage. Accepted for now. |
| §2 Integrity auditing over sealing | ✅ `hash_dir` + `verify_all`; no permission-based sealing |
| §3 Flat-dir model, per-package `bin/<name>/` links | ✅ core done (junction/symlink + copy fallback) · CLI `run`/`remove` migrated |
| ikk-cli migration | ⚠️ compiles clean, all tests green — one runtime bug: local install fails at bin-link step (see HANDOFF.md) |
| Integration tests | ✅ updated to new APIs |

**Next session: start with `HANDOFF.md` at the repo root.**

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

### D. Pure Fetching (`source.rs`)
**Problem:** `source.rs` is currently "heavy" because it handles extraction and archive detection. This makes adding new source types (like Git or S3) difficult because they must also implement extraction logic.

**Recommendation:**
* **The "Raw Fetch" Pattern:** A `Source` should only be responsible for fetching raw bytes. 
* **Introduce an `ArtifactProcessor`:** Move the logic for "detecting archive type $\rightarrow$ extracting $\rightarrow$ picking best binary" into a dedicated lifecycle stage. 
* **Result:** `source.rs` becomes a lightweight interface that is trivial to extend.

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
