# ikk — ਇੱਕ

> one command to manage all your global CLI tools.

Minimal, secure, cross-platform package manager for system-wide CLI tools —
neovim, ripgrep, fd, llama.cpp, and anything that ships as pre-built binaries.

No sudo, no forge lock-in, no version juggling.

## Install

**macOS / Linux:**
```bash
curl -fsSL https://raw.githubusercontent.com/mandeepsmagh/ikk/main/install.sh | sh
~/.ikk/bin/ikk init --remote github.com
```

**Windows (PowerShell):**
```powershell
irm https://raw.githubusercontent.com/mandeepsmagh/ikk/main/install.ps1 | iex
ikk init --remote github.com
```

**From source:**
```bash
git clone https://github.com/mandeepsmagh/ikk && cd ikk
cargo build --release && mkdir -p ~/.ikk/bin && cp target/release/ikk ~/.ikk/bin/
```

## Quick Start

```bash
ikk init                                         # one-time setup, adds ~/.ikk/bin to PATH
ikk install ripgrep --uri BurntSushi/ripgrep     # forge discovery (auto-picks best binary)
ikk install fd --uri sharkdp/fd --binary fd
ikk list                                          # see what's installed
ikk check                                         # verify nothing's tampered
```

## Configuration

`~/.ikk/ikk.toml` — top-level package keys, no `[packages]` prefix:

```toml
[defaults]
remote = "github.com"

[security]
min_release_age_days = 3    # wait 3 days before allowing latest release

[ripgrep]
uri = "BurntSushi/ripgrep"
version = "14.1.1"

[fd]
uri = "sharkdp/fd"

# URL template — direct download, no forge API
[rik]
uri = "https://github.com/nalply/rik/releases/download/{version}/rik-{version}-x86_64-linux.tar.gz"
version = "0.13.0"

# Variant-aware
[llama-cpp]
uri = "https://github.com/ggml-org/llama.cpp/releases/download/{version}/llama-{version}-bin-ubuntu-{variant}-x64.tar.gz"
version = "b5262"
variant = "cuda12"

# Local binary
[mytool]
uri = "file:///home/user/dev/mytool/target/release/mytool"

# Local build
[myproject]
uri = "file:///home/user/dev/myproject"
build = ["cmake -B build -DCMAKE_BUILD_TYPE=Release", "cmake --build build -j"]
```

Then reconcile:
```bash
ikk sync
```

## Package Modes

| URI | Mode | Description |
|-----|------|-------------|
| `owner/repo` | Forge discovery | Auto-resolves via `defaults.remote`, picks best asset for your OS/arch |
| `host/owner/repo` | Forge discovery | Explicit forge host |
| `https://.../{version}...` | URL template | Direct download with `{version}` and optional `{variant}` tokens |
| `file:///path/to/binary` | Local binary | Links as-is, never copies |
| `file:///path/to/source` + `build` | Local build | Runs shell commands in source dir |

## Directory Layout

```
~/.ikk/
  bin/        symlinks → store entries  (on $PATH)
  store/      content-addressed storage ({hash}-{name}-{version}[-{variant}])
  stage/      temporary extraction
  ikk.toml    what you want
  ikk.lock    what's installed (merkle-rooted, never edit)
```

## Commands

```
ikk init [--remote <host>]          one-time setup
ikk install <name> --uri <uri>      install a package
ikk remove <name>                   remove a package
ikk sync                            install/upgrade/remove to match ikk.toml
ikk upgrade [name] [--force]        upgrade to latest
ikk list [name]                     list packages
ikk info <name>                     show package details
ikk run <name> <binary> [-- args]   run a binary from a multi-file package
ikk check                           verify lock integrity + binary hashes
ikk gc [--dry-run]                  remove unused store entries
ikk config get <key>                read a config value
ikk config set <key> <value>        set a config value
ikk self-update [--check]           update ikk itself
ikk uninstall [--yes]               remove everything
```

## New Machine

```bash
ikk init --silent --remote github.com
ikk sync
```

Or bootstrap from a committed lock file:
```bash
ikk init --silent --remote github.com
cp ~/dotfiles/ikk.lock ~/.ikk/
ikk sync
```

## Design

- **Declared state** — `ikk.toml` describes what you want, `ikk sync` makes it real. Idempotent.
- **One version per package** — no shims, no version switching.
- **Forge-agnostic** — GitHub, GitLab, Codeberg, Gitea: all config in `remotes.toml`, zero code.
- **Content-addressed store** — binary hashed before storing, integrity digest over lock file, `ikk check` re-verifies.
- **Multi-binary packages** — directory packages with `ikk run` for tools like llama.cpp.
- **No sudo** — everything in `~/.ikk`.
