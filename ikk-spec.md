# ikk — Design Specification

> A binary package manager focused on versioned, variant-aware installation of
> pre-built and locally built binaries across Unix and Windows.

---

## Goals

- Install pre-built binaries from remote URLs
- Install local binaries directly
- Build from local source using arbitrary shell commands
- Support variants of the same package (cuda12, cuda13, vulkan, cpu)
- Stable unversioned names to depend on regardless of active version
- One active version per package at a time
- Works identically on Unix and Windows (different mechanics, same model)

## Non-Goals

- Cloning or managing git repositories
- Building from source automatically
- Multiple simultaneously active versions of the same package
- A hosted registry (local recipes only)
- Auto-detecting variants from the environment

---

## Files

### ikk.toml

What you want. User edits this.

```toml
[packages.ripgrep]
uri = "https://github.com/BurntSushi/ripgrep/releases/download/{version}/ripgrep-{version}-x86_64-unknown-linux-musl.tar.gz"

[packages.llama-cpp]
uri     = "https://github.com/ggml-org/llama.cpp/releases/download/{version}/llama-{version}-bin-ubuntu-{variant}-x64.tar.gz"
variant = "cpu"

[packages.llama-cpp-cuda]
uri   = "file:///home/user/dev/llama-cpp"
build = ["cmake -B build -DGGML_CUDA=ON", "cmake --build build -j"]

[packages.mytool]
uri = "file:///home/user/dev/mytool/target/release/mytool"
```

### ikk.lock

What is actually installed. Managed by `ikk`, never edited by hand.

```toml
[packages.ripgrep]
version      = "14.1.1"
uri          = "https://github.com/BurntSushi/ripgrep/releases/download/14.1.1/ripgrep-14.1.1-x86_64-unknown-linux-musl.tar.gz"
sha256       = "abc123..."
bin_entry    = "rg-14.1.1"
is_dir       = false
installed_at = 1749600000

[packages.llama-cpp]
version      = "b5262"
variant      = "cuda12"
uri          = "https://github.com/ggml-org/llama.cpp/releases/download/b5262/llama-b5262-bin-ubuntu-cuda12-x64.tar.gz"
sha256       = "def456..."
bin_entry    = "llama-cpp-b5262-cuda12"
is_dir       = true
installed_at = 1749600000

[packages.llama-cpp-cuda]
uri          = "file:///home/user/dev/llama-cpp"
build        = ["cmake -B build -DGGML_CUDA=ON", "cmake --build build -j"]
bin_entry    = "llama-cpp-cuda-local"
is_dir       = true
installed_at = 1749600000

[packages.mytool]
uri          = "file:///home/user/dev/mytool/target/release/mytool"
bin_entry    = "mytool"
is_dir       = false
installed_at = 1749600000
```

---

## Package Modes

Three modes determined by `uri` scheme and presence of `build`.

| `uri` scheme | `build` present | Mode             |
|--------------|-----------------|------------------|
| `https://`   | no              | Remote download  |
| `file://`    | no              | Local binary     |
| `file://`    | yes             | Local build      |

### Remote download

```toml
[packages.ripgrep]
uri = "https://.../ripgrep-{version}-x86_64-unknown-linux-musl.tar.gz"

[packages.llama-cpp]
uri     = "https://.../llama-{version}-bin-ubuntu-{variant}-x64.tar.gz"
variant = "cpu"
```

- `{version}` substituted from `--version` flag at install time
- `{variant}` substituted from `variant` field, overridden by `--variant` flag
- `variant` ignored if `{variant}` not present in uri

### Local binary

```toml
[packages.mytool]
uri = "file:///home/user/dev/mytool/target/release/mytool"
```

- Points directly at a binary
- `ikk` links it as-is, never copies or modifies

### Local build

```toml
[packages.llama-cpp-cuda]
uri   = "file:///home/user/dev/llama-cpp"
build = ["cmake -B build -DGGML_CUDA=ON", "cmake --build build -j"]
```

- `uri` points at source directory
- `build` is an ordered list of shell commands run in that directory
- User owns and manages the source directory — `ikk` never touches it
- If any command fails, install aborts with the error
- `ikk` scans for executables after build completes

---

## Directory Layout

```
~/.ikk/
  bin/        real files, versioned names
  store/      stable named entry points — on $PATH
  cache/      downloaded archives
  ikk.toml    what you want
  ikk.lock    what is installed
```

### bin/

Real installed files. Version and variant encoded in name so two versions
can coexist on disk during an upgrade.

```
bin/
  rg-14.1.1                        single binary
  llama-cpp-b5262-cpu/             multi-file package
    llama-cli
    llama-server
    libggml-cpu.so
  llama-cpp-b5262-cuda12/
    llama-cli
    llama-server
    libggml-cuda.so
```

### store/

Stable named symlinks (Unix) or junctions/shims (Windows). One entry per
package, no version in the name. Only this directory goes on `$PATH`.

```
store/
  rg -> ../bin/rg-14.1.1
  llama-cpp -> ../bin/llama-cpp-b5262-cuda12/
```

Switching versions = re-point the symlink + delete the old bin entry.
Scripts and users always use `store/<name>` regardless of active version.

### cache/

Downloaded archives stored by sha256. Never re-downloaded if already present.
Cleared with `ikk cache clean`.

---

## Linker — Unix vs Windows

Same interface, platform-specific implementation. `$PATH` points to
`~/.ikk/store` on both platforms.

### Unix

- Single binary: `store/rg` → symlink → `../bin/rg-14.1.1`
- Directory: `store/llama-cpp` → symlink → `../bin/llama-cpp-b5262-cuda12/`

### Windows

Symlinks require elevation — not reliable.

- Single binary: `store\rg.bat` shim:
  ```bat
  @echo off
  "%~dp0\..\bin\rg-14.1.1.exe" %*
  ```
- Directory: `store\llama-cpp\` NTFS junction →
  `bin\llama-cpp-b5262-cuda12\`. Junctions require no elevation.

---

## Layers

```
CLI
 │   ikk install / remove / upgrade / list / verify / run
 ▼
Resolver
 │   reads ikk.toml, applies {version} and {variant} substitution
 │   determines mode from uri scheme + build presence
 │   ──► ResolvedPackage { mode, uri, bin_entry, is_dir }
 ▼
Fetcher          (mode = download)
 │   https:// ──► verified archive bytes, cached in cache/
 OR
Builder          (mode = build)
 │   runs build commands in source dir, scans for executables
 OR
 │   (mode = local, no-op — uri is the path)
 ▼
Unpacker         (download only)
 │   archive ──► bin/<bin_entry>/
 ▼
Linker
 │   bin/<bin_entry> ──► store/<name>   (symlink / junction / shim)
 ▼
Lock Writer
     updates ikk.lock
```

---

## CLI

```
ikk install <name>                         # install with defaults
ikk install <name> --version <ver>        # specific version
ikk install <name> --variant <id>         # override variant
ikk install <name> --build                # force build mode
ikk remove  <name>                        # remove and clean bin entry
ikk upgrade <name> [--version] [--variant]# install new, remove old
ikk list                                  # show installed from ikk.lock
ikk verify                                # re-hash all, report mismatches
ikk run <name> <binary> [-- <args>]       # exec into a dir package binary
ikk cache clean                           # remove cached archives
```

`ikk run` is a convenience for multi-file packages. For single binaries
`store/rg` is already on `$PATH` so they work directly without `ikk run`.

---

## Open Questions

1. **`{version}` without `--version`** — must version always be explicit,
   or should `ikk` query the GitHub releases API for latest? Start explicit,
   add API lookup later.

2. **sha256 in ikk.toml** — prebuilt URIs should have a known sha256 for
   verification. Where does it live — in `ikk.toml` per version entry, or
   fetched from a sidecar `.sha256` file alongside the archive?

3. **Build output discovery** — after a build `ikk` scans for executables.
   What if the build produces executables in unexpected places? Optional
   `artifacts` field as an explicit override?

4. **PATH setup** — `ikk` should offer to add `~/.ikk/store` to the shell
   profile on first install. Which shells to support — bash, zsh, fish,
   PowerShell?
