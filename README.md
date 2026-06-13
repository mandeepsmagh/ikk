# ikk — ਇੱਕ

> one tool to manage all packages.

A minimal, secure, cross-platform package manager for pre-built binaries.

---

## Design Principles

- **One active version per package** — no version juggling, no confusion
- **Declared state** (`ikk.toml`) → **exact reality** (`ikk.lock`) → `ikk sync` converges them
- **No forge assumptions** — GitHub, GitLab, Codeberg, self-hosted Forgejo: all config, zero code changes ever
- **No sudo, ever** — everything lives in `~/.ikk`, blast radius contained to your user
- **Supply chain aware** — two hashes per package (archive + binary), merkle root over entire lock
- **Idempotent by design** — run `ikk sync` 100 times, same result

---

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/mandeepsmagh/ikk/main/install.sh | sh
```

Or build from source:
```bash
git clone https://github.com/mandeepsmagh/ikk
cd ikk
cargo build --release
cp target/release/ikk ~/.local/bin/
```

---

## Quick Start

```bash
# initialise — creates ~/.ikk, adds to PATH
ikk init

# add packages
ikk add BurntSushi/ripgrep
ikk add sharkdp/fd
ikk add codeberg.org/helix/helix

# sync everything in ikk.toml to current machine
ikk sync

# upgrade all latest-pinned packages
ikk upgrade

# verify nothing has been tampered with
ikk check
```

---

## Config — `~/.ikk/ikk.toml`

```toml
[defaults]
remote = "github.com"      # default forge host — set once, never repeat

[security]
min_release_age_days = 3   # don't install releases younger than 3 days

[auth.tokens]
"github.com"   = { env = "GITHUB_TOKEN" }    # token read from env, never stored
"gitlab.com"   = { env = "GITLAB_TOKEN" }
"codeberg.org" = { env = "CODEBERG_TOKEN" }

# ssh key for private repo builds (optional)
# ssh_key = "~/.ssh/id_ed25519"

# packages — source is the only required field
[packages.ripgrep]
source  = "BurntSushi/ripgrep"    # uses defaults.remote
version = "latest"

[packages.helix]
source  = "codeberg.org/helix/helix"   # explicit host
version = "latest"

[packages.mytool]
source  = "github.com/me/mytool"
version = "1.2.0"                 # pinned — never auto-upgraded

[packages.localtool]
source  = "~/Downloads/localtool.tar.gz"
version = "2.0.0"
binary  = "localtool"

[packages.myproject]
source  = "~/projects/myproject"
version = "dev"
build   = { system = "cargo", binary = "myproject" }

# add your own forge — no ikk update required
[[remotes]]
host            = "git.mycompany.com"
releases_url    = "https://git.mycompany.com/api/v1/repos/{owner}/{repo}/releases"
version_path    = "tag_name"
prerelease_path = "prerelease"
draft_path      = "draft"
assets_path     = "assets"
asset_url_path  = "browser_download_url"
asset_name_path = "name"
auth_env        = "WORK_GIT_TOKEN"
```

---

## Lock File — `ikk.lock`

Generated automatically. Commit this to dotfiles for reproducible environments.

```toml
tree_root = "a3f4b5c6d7e8..."    # merkle root — detects any tampering

[packages.ripgrep]
version        = "14.1.1"
source_url     = "https://github.com/BurntSushi/ripgrep"
download_url   = "https://github.com/.../ripgrep-14.1.1-aarch64-apple-darwin.tar.gz"
archive_sha256 = "e3b0c44298fc1c149a..."
binary_sha256  = "a665a45920422f9d17..."
store_hash     = "a665a45920"
```

---

## Directory Layout

```
~/.ikk/
  ikk.toml          ← your config (edit this)
  ikk.lock          ← generated, commit to dotfiles
  store/            ← content-addressed, read-only after install
    a665a45920-ripgrep-14.1.1/
      bin/rg        ← sealed 555
      meta.toml     ← provenance record
  bin/              ← symlinks on PATH
    rg → ../store/a665a45920-ripgrep-14.1.1/bin/rg
  stage/            ← temp during download+verify, always empty after sync
```

---

## Commands

```
ikk init [--silent] [--remote <host>] [--shell <shell>] [--no-shell] [--dry-run]
ikk sync
ikk add <source> [--name <name>] [--version <version>] [--binary <binary>]
ikk remove <name>
ikk upgrade [name] [--force]
ikk check
ikk info <name>
ikk self-update [--check]
ikk config get <key>
ikk config set <key> <value>
ikk uninstall [--yes]
```

---

## New Machine Bootstrap

```bash
# in your dotfiles bootstrap script
ikk init --silent --remote github.com
ikk sync --lock ~/dotfiles/ikk.lock
```

One command. All tools restored at verified versions.

---

## Source Formats

```toml
source = "BurntSushi/ripgrep"                    # owner/repo — uses defaults.remote
source = "codeberg.org/helix/helix"              # host/owner/repo
source = "https://github.com/BurntSushi/ripgrep" # full URL
source = "~/Downloads/tool.tar.gz"               # local archive
source = "~/projects/mytool"                     # local build
```

---

## Workspace

```
ikk/
├── ikk-core/    ← pure domain logic — zero forge knowledge
│   └── src/
│       ├── config.rs    ← Config, SecurityConfig, AuthConfig
│       ├── error.rs     ← IkkError
│       ├── extract.rs   ← archive extraction (tar.gz, tar.xz, zip, dmg, msi, raw)
│       ├── home.rs      ← IkkHome — ~/.ikk layout
│       ├── lock.rs      ← LockFile, merkle root
│       ├── ops.rs       ← install, remove, sync (parallel), self_uninstall
│       ├── platform.rs  ← Platform, asset scoring
│       ├── registry.rs  ← ConfigRegistry — config-driven forge dispatch
│       ├── remote.rs    ← Remote trait, ConfiguredRemote, RemoteConfig
│       ├── remotes.toml ← built-in forge definitions (compiled in)
│       ├── shell.rs     ← shell detection, rc file integration
│       ├── source.rs    ← Source trait — unified local + remote install
│       └── store.rs     ← content-addressed store, verify
│
└── ikk-cli/     ← thin clap shell — wires everything together
    └── src/
        ├── main.rs
        └── commands/
            ├── add.rs
            ├── check.rs
            ├── config.rs
            ├── info.rs
            ├── init.rs
            ├── remove.rs
            ├── self_update.rs
            ├── sync.rs
            ├── uninstall.rs
            └── upgrade.rs
```

---

## Coming Next

- **Per-project config** — `ikk.toml` in a project directory pins tool versions for that
  project. `ikk sync` writes symlinks to `.ikk/bin/` (not `~/.ikk/bin`), so each project
  gets its own tool versions without re-installing. Same content-addressed store, same
  lock file, just different symlink targets. Shell hook (`ikk hook zsh`) adds `.ikk/bin`
  to PATH before the global `~/.ikk/bin` — project versions win, no shims needed.
- **Provenance verification** — cryptographic proof that a binary came from its declared
  source (not just "same bits as last time"). Must work across all forges — GitHub, GitLab,
  Codeberg, self-hosted. Research and implement once a forge-agnostic standard emerges.
  No dependency on proprietary attestation services.
- `ikk search` — search packages across configured forges
- MSI extraction on Windows
- `.deb` / `.rpm` extraction
