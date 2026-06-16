# Code Review: ikk Installation and Execution Issues

This document summarizes the critical bugs identified during the debugging of `ikk add` and execution issues in WSL2.

## 1. Incorrect Binary Selection (The "Icon" Bug)
**Symptom:** `ikk add` installs a tiny, non-executable file (e.g., a 64-byte icon) instead of the actual binary.
**Root Cause:** The `name_match_score` function in `ikk-core/src/extract.rs` uses a greedy `starts_with` matching strategy. When a package name (e.g., `neovim`) is a prefix of other files in the archive (e.g., `neovim-icon.ico`), the "trash" file is selected as the "best match" before the actual binary is processed.
**Fix:** 
- Implement a **"Unpack-then-Search"** pattern in `extract_tar_archive`.
- Unpack the entire archive to a temporary directory first.
- Perform a deterministic search for the file matching the exact `binary_name`.

## 2. Package Name vs. Binary Name Mismatch
**Symptom:** `ikk add neovim` installs successfully, but the `neovim` command is not found.
**Root Cause:** `ikk` defaults the binary name to the package name if `--binary` is not provided. In the case of Neovim, the package is `neovim`, but the executable inside the archive is `nvim`. Because the name mismatch was combined with the "greedy matching" bug, `ikk` could not find the correct file.
**Fix:**
- Ensure the `extract` logic can find the binary even if the package name and filename differ by searching for the file explicitly.
- (User side) Always use `ikk add <repo> --binary <name>` if they differ.

## 3. Read-Only Runtime Error (The "Seal" Bug)
**Symptom:** Installed applications (like Neovim) fail with `Exec format error` or `Read-only file system` when attempting to access or modify syntax/runtime files.
**Root Cause:** The `seal_dir` function in `ikk-core/src/store.rs` applies `0o555` permissions to the entire package directory. This prevents the application from performing standard operations like updating runtime caches, syntax files, or plugins.
**Fix:**
- Remove the `seal_dir` call in `ikk-core/src/store.rs`.
- **Only** call `seal` on the specific binary executable itself to prevent tampering, while leaving the runtime/library directories writable.

---
*Review conducted on: 2026-06-16*
