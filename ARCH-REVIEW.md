
# ikk Package Manager — Architecture Review and Staged Hardening

You are reviewing and improving the `ikk` package manager.

Do **not** blindly implement the items below. First inspect the complete codebase, trace the relevant execution paths, existing tests, platform-specific behavior, crate versions, and invariants. For every proposed change:

1. Confirm whether the concern is actually valid in the current codebase.
2. Explain its practical impact and severity.
3. Check whether another part of the code already mitigates it.
4. Reject or modify the proposal if it would add unnecessary complexity.
5. Only then implement worthwhile changes.
6. Implement in small stages with tests after each stage.
7. Preserve the project's intentionally minimal design.

## Core architecture that should remain

The intended model is:

```text
~/.ikk/
├── bin/              # single directory added to PATH
├── store/            # content-addressed package store
├── stage/            # temporary processing area
├── ikk.toml
└── ikk.lock
```

Packages are:

```text
source
  ↓
fetch raw content
  ↓
extract / normalize package root
  ↓
store complete package tree in CAS
  ↓
automatically discover executable candidates
  ↓
symlink executables into ~/.ikk/bin
  ↓
record exact exported paths in ikk.lock
```

Important product requirements:

* Keep a **single flat `~/.ikk/bin` PATH namespace**.
* It is acceptable for packages such as `llama.cpp` to expose many executables there.
* `~/.ikk/bin` is completely owned/managed by ikk, so having many symlinks is not considered clutter.
* Keep installation **zero configuration** for normal packages.
* Do not require users to manually inspect archives or specify `bins = [...]`.
* Preserve author-provided binary names.
* Preserve the complete upstream package tree in the store.
* Continue automatically discovering executables.
* Continue rejecting command-name collisions between packages.
* The lock file should remain authoritative for which commands an installed package owns.
* Avoid introducing manifests, shims, wrappers, package-specific PATH directories, or other abstractions unless there is a demonstrated need.

The goal is to **harden the existing architecture**, not redesign it.

---

# Deliverables for each stage

Before implementation, report:

```text
Finding:
Status:
Evidence:
Impact:
Recommended change:
Complexity/risk:
Tests needed:
```

Then implement only accepted findings.

After implementation, report:

```text
Files changed:
Behavior changed:
Tests added/updated:
Tests run:
Result:
Remaining risks:
Deferred items:
```

Keep stages independently reviewable.

Do not bundle unrelated cleanup with correctness fixes.


# Stage 0 — Review the existing implementation

Before changing code, trace at least these areas:

* source fetching
* archive selection
* archive extraction
* package-root normalization
* content hashing
* CAS insertion
* store-hit handling
* executable classification
* recursive executable discovery
* PATH export filtering
* symlink handling
* copy fallback
* command collision handling
* upgrades
* uninstall
* lock-file ownership
* store locking / concurrency
* Windows-specific behavior
* macOS-specific behavior
* Linux/Unix-specific behavior

Identify the exact files/functions involved.

Run the current test suite before modifications and establish a clean baseline.

For each issue below classify it as:

```text
CONFIRMED
PARTIALLY VALID
NOT APPLICABLE
ALREADY MITIGATED
NOT WORTH COMPLEXITY
```

Provide reasoning based on actual code.

Do not begin major implementation until this review is complete.

---

# Stage 1 — Executable discovery correctness

Review the current recursive executable discovery mechanism.

The intended behavior is still:

```text
walk package tree
    ↓
identify runnable files
    ↓
only export files allowed by PATH-export policy
    ↓
flatten their basename into ~/.ikk/bin
```

Do **not** replace this with mandatory `bin`-directory discovery unless the current implementation proves fundamentally unsuitable.

## 1.1 Duplicate executable basenames inside one package

Check whether a package containing:

```text
bin/foo
tools/bin/foo
```

can result in one silently replacing the other in the in-memory `BTreeMap`.

If current code effectively does:

```rust
bins.insert(name, path);
```

without detecting an existing value, determine whether filesystem traversal ordering could decide which executable wins.

If confirmed:

* do not silently choose based on `read_dir()` order;
* either reject the package as ambiguous, or establish an explicit deterministic precedence rule;
* prefer rejection unless there is a compelling usability reason for precedence.

Add tests.

Suggested invariant:

> One package must not silently export two different files under the same command name.

## 1.2 Review PATH export filtering

Inspect the logic that determines whether a discovered executable is eligible for `~/.ikk/bin`.

If it currently checks whether **any path component** is named `bin`, verify whether the executable filename itself could satisfy that condition.

For example:

```text
some-directory/bin
```

where `bin` is the executable filename.

If confirmed, make the check apply to parent directory components rather than the filename.

Preserve support for:

```text
foo
bin/foo
build/bin/foo
nested/.../bin/foo
```

if that is intentional current behavior.

Add focused tests.

---

# Stage 2 — Make executable linking transactional

Review the complete upgrade/link flow.

Determine whether the current implementation modifies existing links before all validation has succeeded.

Specifically check:

* stale-link removal;
* collision detection;
* duplicate validation;
* symlink-target validation;
* link creation;
* copy fallback;
* lock update.

Look for a sequence such as:

```text
remove stale old links
    ↓
discover collision/error
    ↓
return error
```

which could leave an otherwise-working installed package partially modified.

If confirmed, restructure around:

```text
DISCOVER
    ↓
VALIDATE
    ↓
PLAN
    ↓
COMMIT
```

Before filesystem mutation, determine:

* links to remove;
* links to replace;
* links to create;
* collisions;
* invalid targets;
* duplicate basenames;
* all other errors detectable in advance.

Only mutate `~/.ikk/bin` after validation succeeds.

Evaluate whether temporary-link + atomic rename is worthwhile. Use it only if it materially improves correctness without excessive cross-platform complexity.

Required invariant:

> A failed upgrade should leave the previously installed command set usable whenever reasonably possible.

Add tests that intentionally cause upgrade failure and verify old links remain intact.

---

# Stage 3 — Package symlink containment

The store intentionally preserves package symlinks.

Review what happens when executable discovery encounters a package-provided symlink such as:

```text
bin/foo -> ../libexec/foo
```

versus:

```text
bin/foo -> /usr/bin/foo
```

or:

```text
bin/foo -> ../../../../outside/package
```

Trace:

```text
stored package symlink
    ↓
is_runnable()
    ↓
link_file()
    ↓
~/.ikk/bin/foo
```

Determine whether an exported ikk command can ultimately resolve outside the package's store root.

If it can, decide whether that is an unacceptable trust-boundary violation.

Preferred policy if confirmed:

* preserve all upstream symlinks in the CAS;
* allow executable symlinks whose final resolved target remains within that package root;
* do not export package symlinks that escape the package root.

Be careful about:

* relative links;
* chained symlinks;
* broken links;
* symlink loops;
* Windows symlink/junction behavior;
* TOCTOU concerns;
* canonicalization behavior.

Do not accidentally reject legitimate internal package symlinks.

Add tests for:

```text
internal relative executable symlink        -> allowed
external absolute executable symlink        -> rejected/not exported
relative escape outside package             -> rejected/not exported
broken symlink                              -> fail closed
cycle                                       -> fail closed
```

---

# Stage 4 — Harden executable format classification

Review `binary::is_runnable()` against the actual supported platforms.

The classifier should remain content-based where appropriate because Unix execute bits alone are not sufficient to distinguish executables from shared libraries.

Validate each concern below before changing anything.

## 4.1 Host operating-system format validation

Check whether all `unix` targets currently recognize both:

* ELF
* Mach-O

If so, a Linux host may classify a Mach-O executable as runnable and macOS may classify an ELF executable as runnable.

If confirmed, constrain executable formats to operating systems where they can actually run.

Do not create broad unsupported assumptions about every Unix variant.

## 4.2 Architecture validation

Determine whether ELF `e_machine` and Mach-O CPU type are checked.

If not, an ARM executable may be classified as runnable on x86_64 or vice versa.

If practical within the supported platform matrix, validate host architecture.

Fail closed on unsupported architectures.

## 4.3 Fat Mach-O

Check whether fat/universal Mach-O currently examines only the first architecture slice.

If confirmed, determine whether it should instead locate a slice compatible with the host architecture.

Add tests using synthetic headers if real fixture binaries are undesirable.

## 4.4 ELF parsing robustness

Review:

* ELF class;
* byte order;
* `ET_EXEC`;
* `ET_DYN`;
* `PT_INTERP`;
* program-header sizes;
* offsets;
* counts;
* truncated input.

Use checked arithmetic for offsets:

```rust
checked_mul
checked_add
```

Package files are untrusted input.

Do not over-engineer obscure ELF extensions unless they are useful to ikk's supported packages.

Static PIE support should be evaluated, not assumed necessary.

## 4.5 Windows extensions

Review whether directly runnable Windows command formats are correctly represented.

At minimum evaluate:

```text
.exe
.com
.bat
.cmd
```

But preserve ikk's intended Windows execution semantics and avoid expanding into every `PATHEXT` type without need.

---

# Stage 5 — CAS insertion atomicity

Review `Store::insert()` carefully.

Determine whether the final store entry directory is created before the package has been completely copied and metadata written.

Evaluate this failure mode:

```text
create final store/<entry>/
    ↓
start copying
    ↓
process killed / system crash
    ↓
partial directory remains
    ↓
next install sees entry.exists()
    ↓
treats it as a store hit
```

Rust error cleanup does not protect against process termination or power loss.

If confirmed, implement an atomic store-commit model such as:

```text
store/.tmp-<unique>/
    ↓
copy complete package
    ↓
write metadata
    ↓
optional validation
    ↓
rename atomically
    ↓
store/<final-entry>/
```

Requirements:

* temporary names must be collision resistant;
* concurrent install behavior must remain correct;
* preserve existing store locking assumptions;
* clean temporary entries when practical;
* final store entries should only become visible after they are complete.

Add failure-oriented tests where feasible.

Required invariant:

> Presence of a final store entry means the entry is complete enough to use.

---

# Stage 6 — Validate full content hash on store hits

The store path currently appears to use a shortened SHA-256 prefix for readability.

Review whether an existing directory such as:

```text
<hash12>-<name>-<version>
```

is automatically accepted as a cache/store hit without validating its full stored content hash.

If confirmed, retain the short path if desired, but verify `meta.toml` contains the expected full:

```text
content_sha256
```

before declaring a hit.

Handle:

* truncated/corrupt metadata;
* mismatched full hashes;
* partial entries;
* extremely unlikely prefix collision;
* race with another installation.

Do not silently treat a different full hash as the same object.

Add tests.

---

# Stage 7 — ZIP extraction hardening

Review ZIP extraction independently of tar extraction.

## 7.1 Path containment

Inspect the installed version of the `zip` crate.

If it provides a vetted method such as `enclosed_name()`, prefer that over custom path sanitization where appropriate.

Test path traversal on both Unix-style and Windows-style paths, including:

```text
../evil
../../evil
/absolute/path
C:\absolute\path
..\windows\escape
```

The invariant is:

> No archive entry may write outside the extraction root.

Do not assume Unix path semantics on Windows.

## 7.2 Preserve executable modes

Check whether ZIP extraction currently discards Unix permission metadata.

If the ZIP entry exposes `unix_mode()`, preserve relevant mode bits on Unix.

This matters because a package may correctly ship:

```text
0755 bin/foo
```

and ikk should not normalize it into `0644`.

Add tests if the ZIP crate makes synthetic fixtures practical.

## 7.3 Symlinks

Review how ZIP symlink entries are represented by the crate and what current code does with them.

Do not add symlink extraction support merely for completeness unless real package compatibility requires it.

If supported, apply the same extraction containment/security model as tar packages.

---

# Stage 8 — Improve CAS tree hashing

Review whether the current package-content hash is canonical enough for the semantics ikk wants.

Do not change the hash format casually because doing so invalidates existing store identities. Treat this as a versioned/migration-sensitive change.

Evaluate the following separately.

## 8.1 File permissions

Check whether these currently hash identically:

```text
-rwxr-xr-x foo
-rw-r--r-- foo
```

If yes, decide whether executable/read/write mode bits are part of package identity.

For a package manager, including meaningful Unix permissions is probably valuable.

Possible choices:

```text
mode & 0o777
```

or, if only execution semantics matter:

```text
mode & 0o111
```

Choose deliberately and document the invariant.

## 8.2 Streaming file hashing

If current hashing performs:

```rust
std::fs::read(path)
```

for entire files, replace with streaming hashing if it improves memory behavior for large binaries.

Avoid unnecessary double hashing such as:

```text
file bytes
    ↓ sha256 hex string
    ↓ hash hex string into directory hash
```

unless this is intentionally part of the existing format.

## 8.3 Domain separation / canonical encoding

Evaluate whether the tree hash should distinguish:

```text
FILE
DIRECTORY
SYMLINK
```

and encode names/values unambiguously with lengths or stable delimiters.

Example conceptual encoding:

```text
FILE | name-length | name | mode | content-hash
DIR  | name-length | name | child-hash
LINK | name-length | name | target-length | target
```

Do not implement a complex Merkle format unless the benefits justify the migration.

If the existing hash is adequate for ikk's threat model, explain that and leave it alone.

---

# Stage 9 — Review lock-file integrity semantics

Inspect `LockFile::compute_root()`.

Confirm exactly which fields are covered.

Check documentation against implementation.

If fields are concatenated directly:

```rust
name + version + uri + ...
```

evaluate whether length-prefixing or another unambiguous encoding is worthwhile.

Also clarify the security property.

The lock-file tree root is useful for:

* accidental corruption detection;
* detecting unsynchronized edits when the root isn't recomputed.

It does **not** authenticate against a malicious local actor who can modify both the lock contents and its digest.

Documentation should not overstate this guarantee.

Do not introduce signing/key infrastructure.

---

# Stage 10 — Terminology cleanup

Only after functional work is stable, evaluate historical names that make the architecture harder to understand.

Examples currently worth reviewing:

```text
PACKAGE_DIR = "bin"
```

when that directory actually contains the **entire package tree**.

Possible replacement:

```text
package
```

or:

```text
root
```

Similarly:

```text
bin_entry
```

appears to represent a store entry rather than a binary entry.

Possible replacement:

```text
store_entry
```

If renaming persisted lock fields:

* maintain backward compatibility;
* use serde aliases or explicit migration as appropriate;
* don't break existing installations unnecessarily.

This is cleanup, not a high-priority functional change.

---

# Stage 11 — Other small issues to validate

Review these only if still applicable.

## Relative local paths

Determine whether:

```text
./project
../project
```

are correctly classified as local sources.

If not, support them without breaking forge shorthand such as:

```text
owner/repo
```

## Source provenance naming

Check whether `source_url` sometimes stores only an asset filename rather than an actual URL.

If so, decide whether this is merely naming confusion or lost provenance worth fixing.

Do not expand scope unnecessarily.

---

# Implementation order

After completing Stage 0, propose the actual implementation plan based on confirmed findings.

Preferred rough priority:

## P0 — correctness / safety boundaries

* exported symlink containment, if confirmed;
* transactional executable relinking/upgrades;
* atomic CAS insertion;
* archive extraction containment.

## P1 — deterministic behavior

* duplicate executable basename handling;
* PATH export-filter correctness;
* host OS/architecture executable validation;
* full-hash verification on store hits;
* ZIP executable permission preservation.

## P2 — robustness / maintainability

* streaming and metadata-aware tree hashing;
* lock hashing cleanup;
* terminology cleanup;
* provenance/local-path cleanup.

Do not force this ordering if code dependencies suggest a better one.

---

# Testing expectations

Each stage should include focused tests.

Prefer tests that exercise invariants rather than implementation details.

Important cases include:

```text
single raw executable
root-level executable
nested bin executable
multiple binaries from one package
same command from two packages
same basename twice inside one package
upgrade removes an executable
upgrade adds an executable
failed upgrade preserves previous working links
broken package symlink
internal package symlink
external package symlink
truncated ELF
wrong-architecture ELF
wrong-OS executable format
fat Mach-O host slice
ZIP traversal
ZIP Unix permissions
store prefix/full-hash mismatch
partial/corrupt store entry
uninstall removes only owned links
```

Run:

* formatting;
* lints;
* unit tests;
* integration tests;
* platform-specific tests that are practical in the current environment.

Do not weaken tests merely to make changes pass.

---

---

# Final architectural invariants

When finished, ikk should retain these properties:

1. One `~/.ikk/bin` directory is added to PATH.
2. Package executables are discovered automatically.
3. Packages may expose multiple executables.
4. Executables keep upstream names.
5. Cross-package basename collisions are never silently overwritten.
6. Same-package ambiguity is deterministic or rejected.
7. The complete upstream package tree is preserved in CAS.
8. PATH entries point back to package content in the store whenever symlinks are supported.
9. A failed upgrade should not unnecessarily break the previous install.
10. A visible final CAS entry should be complete and validated.
11. Package-controlled symlinks must not unexpectedly escape ikk's intended trust boundary.
12. Archive extraction must remain inside its staging directory.
13. `ikk.lock` records the exact installed executable mapping so upgrades/uninstalls do not need rediscovery.
14. Normal installation remains zero-config.
15. Security hardening should not turn ikk into a substantially more complex package manager.

Prefer the smallest implementation that satisfies these invariants.
