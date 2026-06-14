# ikk — ਇੱਕ

> one command to manage all packages.

Minimal, secure, cross-platform package manager for pre-built binaries. No sudo, no forge lock-in, no version juggling.

## Install

**macOS / Linux:**
```bash
curl -fsSL https://raw.githubusercontent.com/mandeepsmagh/ikk/main/install.sh | sh
~/.ikk/bin/ikk init --remote github.com
```

**Windows (PowerShell):**
```powershell
irm https://raw.githubusercontent.com/mandeepsmagh/ikk/main/install.ps1 | iex
# open a new terminal, then:
ikk init --remote github.com
```

**Build from source:**
```bash
git clone https://github.com/mandeepsmagh/ikk && cd ikk
cargo build --release && mkdir -p ~/.ikk/bin && cp target/release/ikk ~/.ikk/bin/
```
```

## Quick Start

```bash
ikk init --remote github.com           # one-time setup
ikk add BurntSushi/ripgrep             # add + install immediately
ikk add sharkdp/fd
ikk sync                               # or add entries to ikk.toml and sync all at once
ikk list                               # see what's installed
ikk check                              # verify nothing's tampered
```

## Design

- **Declared state** — `ikk.toml` describes what you want, `ikk sync` makes it real. Idempotent.
- **One version per package** — no shims, no version switching, no confusion.
- **Forge-agnostic** — GitHub, GitLab, Codeberg, self-hosted Forgejo: all config, zero code.
- **Content-addressed store** — two hashes per package, merkle root over entire lock file. `ikk check` re-verifies everything.
- **No sudo** — everything in `~/.ikk`.

## Commands

```
ikk init [--remote <host>]        one-time setup — creates ~/.ikk, adds to PATH
ikk add <source> [--version <v>]  add a package (owner/repo, url, or local path)
ikk remove <name>                 remove a package
ikk sync                          install/upgrade/remove to match ikk.toml
ikk upgrade [name] [--force]      upgrade to latest versions
ikk list [name]                   list packages and install status
ikk info <name>                   show package details
ikk check                         verify lock integrity and binary hashes
ikk gc [--dry-run]                remove unused packages from the store
ikk config get <key>              read a config value
ikk config set <key> <value>      set a config value
ikk self-update [--check]         update ikk itself
ikk uninstall [--yes]             remove everything
```

## Source Formats

```toml
source = "BurntSushi/ripgrep"                     # owner/repo — uses defaults.remote
source = "codeberg.org/helix/helix"               # host/owner/repo
source = "https://github.com/BurntSushi/ripgrep"  # full URL
source = "~/Downloads/tool.tar.gz"                # local archive
source = "~/projects/mytool"                      # local build
```

## New Machine

```bash
ikk init --silent --remote github.com
ikk sync --lock ~/dotfiles/ikk.lock
```

One command. All tools restored at verified versions from a committed lock file.

## Coming Next

- Per-project config with `.ikk/bin` directories
- Provenance verification (forge-agnostic, not yet standardized)
- MSI extraction on Windows
- `.deb` / `.rpm` extraction on Linux
