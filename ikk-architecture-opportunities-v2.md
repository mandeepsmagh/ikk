# ikk Architecture: Minimal Core, Composable Tooling

## Purpose

`ikk` is a minimal, upstream-first package manager for fast-moving tools that already publish usable release artifacts.

Its goal is not to recreate Homebrew, Nix, containers, or a centralized package ecosystem. The core idea is simpler:

> Resolve tools directly from upstream, store them once in a local content-addressed store, and compose access to them only when needed.

This keeps the core small while leaving room for project environments, ephemeral tasks, agents, CI, containers, microVMs, editors, schedulers, and future consumers.

---

## 1. Current Architecture

```text
Source / upstream release
        ↓
Resolve version + platform asset
        ↓
Fetch raw content
        ↓
Extract / normalize package root
        ↓
Local content-addressed store
        ↓
Discover runnable executables
        ↓
~/.ikk/bin symlinks
        ↓
ikk.lock ownership + exact paths
```

Default home layout:

```text
~/.ikk/
├── bin/          # global commands on PATH
├── store/        # local CAS, complete package trees
├── stage/        # temporary processing
├── ikk.toml      # requested state
└── ikk.lock      # resolved state + command ownership
```

### Current design principles

- Use upstream releases as the primary source of truth.
- Prefer zero-config installation.
- Preserve the complete package tree.
- Keep author-provided executable names.
- Store package contents locally and immutably where practical.
- Use a single flat `~/.ikk/bin` for global installs.
- Allow packages to expose multiple executables.
- Reject command-name collisions instead of silently shadowing.
- Record exact command ownership in the lock file.
- Avoid centralized package recipes unless a real need appears.

This is especially useful for fast-moving projects such as `llama.cpp`, where upstream releases can move faster than Homebrew, Nix, distro repositories, or other curated package ecosystems.

---

## 2. Core Abstraction for Future Growth

Avoid making "shell", "project environment", "Docker", or "agent" the core abstraction.

Use a more neutral model:

```text
Requirements
    +
Target
    ↓
Resolution
    ↓
CAS
    ↓
Projection
    ↓
Consumer
```

### Requirements

What tools are needed.

Examples:

```text
cmake 4.1
ninja 1.13
llama.cpp latest
jq 1.8
```

Requirements may come from:

- global config
- project config
- a temporary command
- an agent task
- CI
- an editor
- another program

### Target

Where the resolved artifacts must run.

At minimum:

```text
Target {
    os
    arch
}
```

Potentially later:

```text
Target {
    os
    arch
    abi
    libc
}
```

The important rule is:

> Resolution should not permanently assume "current host".

This enables the same machine to cache artifacts for:

```text
macOS / arm64
Linux / arm64
Linux / amd64
```

which becomes valuable for containers, cross-platform CI, remote execution, and microVMs.

#### Target selection

The CLI should default to the current host platform:

```text
effective target =
    explicit --target
    OR
    detected host platform
```

Examples:

```bash
ikk add neovim
# target = current host

ikk add neovim --target linux/arm64
# target = linux/arm64
```

After target selection, downstream resolution should not care where the target came from. A target may eventually come from the host, CLI, project configuration, CI, an agent, container tooling, or another consumer.

Targets should be resolved and cached on demand. Cross-target support does **not** mean downloading every platform artifact in a release; ikk should fetch only the artifact required for the effective target.

### Resolution

The exact selected package set:

```text
package
version
variant
platform asset
content/store identity
commands
```

A resolution should be deterministic enough to lock, reproduce, inspect, and materialize again.

### CAS

The shared local content store.

```text
~/.ikk/store/
```

This remains the primary runtime store because it provides:

- real filesystem paths
- executable permissions
- symlinks
- adjacent resources
- shared libraries
- mmap / native execution
- offline use
- cheap reuse across consumers

Object storage may later be useful as an optional remote cache, but the local filesystem should remain the execution store.

### Projection

A projection is a derived filesystem view of a resolution.

The simplest projection is:

```text
view/
└── bin/
```

Later a projection could contain:

```text
view/
├── bin/
├── lib/
├── include/
└── share/
```

or be exported as:

```text
directory
tar archive
OCI layer
VM/rootfs input
```

The CAS stays unchanged; only the projection changes.

### Consumer

Anything that uses a projection.

Examples:

```text
global PATH
project shell
single command
script
Make / just
CI job
agent
editor
scheduler
Docker / Podman
Apple container runtime
microVM
remote worker
future integrations
```

Consumers should adapt to resolved CAS objects and projections. The core should not grow a dedicated subsystem for every consumer.

---

## 3. Global Installs

Global installs remain intentionally simple:

```text
resolution
    ↓
CAS
    ↓
persistent projection
    ↓
~/.ikk/bin
```

Example:

```text
~/.ikk/bin/
├── nvim         -> ~/.ikk/store/.../package/bin/nvim
├── llama-cli    -> ~/.ikk/store/.../package/build/bin/llama-cli
└── llama-server -> ~/.ikk/store/.../package/build/bin/llama-server
```

`~/.ikk/bin` is reserved for global installs and is the only ikk-managed directory added permanently to `PATH`.

---

## 4. Project-Scoped Tooling

Project tooling should not mutate `~/.ikk/bin`.

A project can contain:

```text
project/
├── ikk.toml
├── ikk.lock
├── justfile
└── src/
```

The project resolution can materialize a disposable or cached projection:

```text
~/.ikk/views/<hash>/
└── bin/
    ├── cmake -> CAS
    ├── ninja -> CAS
    └── llama-cli -> CAS
```

Then:

```bash
ikk shell
```

can launch a shell with:

```text
project view/bin
    ↓
~/.ikk/bin
    ↓
system PATH
```

This allows multiple projects to use different versions simultaneously without changing global state.

Useful future commands:

```bash
ikk sync
ikk shell
ikk exec -- <command>
ikk env
```

These are conveniences over the same underlying resolution and projection model, not separate package systems.

---

## 5. Ephemeral Execution

`ikk exec` is a strong generic primitive.

Example:

```bash
ikk exec --with jq -- jq '.foo' data.json
```

or:

```bash
ikk exec \
  --with cmake \
  --with ninja \
  -- just build
```

Conceptually:

```text
current requirements
    +
temporary requirements
    ↓
derived resolution
    ↓
CAS hit or fetch
    ↓
temporary projection
    ↓
execute command
    ↓
discard projection/reference
```

The package objects may remain cached in CAS, so repeated temporary tasks become cheap.

This works for:

- one-off scripts
- code generation
- media conversion
- diagnostics
- project tasks
- CI
- agents
- scheduled jobs

without globally installing tools.

---

## 6. Make, just, Scripts, and CI

Because ikk ultimately exposes normal executables, integration should stay boring.

Example `justfile`:

```make
setup:
    ikk sync

build:
    ikk exec -- cmake -B build
    ikk exec -- cmake --build build

test:
    ikk exec -- just run-tests
```

Or inside `ikk shell`:

```bash
just build
```

CI:

```bash
ikk sync
ikk exec --locked -- just test
```

The task runner remains independent of ikk.

---

## 7. Native Scheduling

Do not build a scheduler into the core.

Use native scheduling:

```text
launchd
systemd timers
cron
Windows Task Scheduler
```

and let scheduled jobs invoke:

```bash
ikk exec --with <tool> -- <script>
```

Separation of concerns:

```text
Scheduling        → OS
Tool resolution   → ikk
Execution context → ikk
Task logic        → script/program
```

This avoids duplicating platform-specific scheduling semantics while still allowing scheduled jobs to use cached, versioned tools.

---

## 8. Agents

Agentic use is one of the strongest reasons to keep the core generic.

An agent may request:

```text
ripgrep
ast-grep
jq
cmake
protoc
```

without modifying global or project state.

Flow:

```text
agent requirements
    ↓
target
    ↓
ephemeral resolution
    ↓
CAS
    ↓
projection
    ↓
agent subprocesses
```

Benefits:

- no system package mutation
- no manual setup
- cheap repeated tasks
- reproducible tool versions
- isolated task-specific capability sets
- easy provenance through resolution hashes
- natural fit with sandboxes, containers, or microVMs

A future agent API should consume the same resolver and projection primitives as the CLI, not a separate package subsystem.

---

## 9. Containers

Docker, Podman, and similar runtimes should be treated as consumers.

Example:

```text
requirements
    ↓
target = linux/arm64
    ↓
resolution
    ↓
CAS
    ↓
container projection
    ↓
Docker / Podman / Apple container runtime
```

Possible uses:

- bind-mount cached CAS content read-only
- generate container-specific symlink views
- reuse tools during image builds
- provide BuildKit cache inputs
- export a projection as a build context
- later export deterministic tar or OCI layers

Important:

> Do not make Docker-specific logic part of package resolution.

The same resolved objects should be usable by any compatible container runtime.

---

## 10. MicroVMs and Sandboxes

The same model extends naturally to microVMs:

```text
task requirements
    ↓
target = linux/arm64
    ↓
resolution
    ↓
CAS
    ↓
filesystem/rootfs projection
    ↓
microVM
```

Potential consumers include:

- Linux microVM runtimes
- Apple virtualization/container frameworks
- ephemeral CI workers
- agent sandboxes
- remote execution systems

A VM can disappear after the task while the CAS remains warm.

This enables cheap isolated execution without reinstalling the toolchain every time.

---

## 11. Cross-Target Caching

A major future opportunity is keeping multiple target artifacts in the same store:

```text
store/
├── cmake-linux-amd64
├── cmake-linux-arm64
├── cmake-macos-arm64
├── ninja-linux-amd64
└── ninja-linux-arm64
```

This enables a macOS workstation to prepare or cache tools for Linux containers or remote workers without changing the host installation.

The key architectural requirement is to make target platform an explicit input to resolution.

---

## 12. Remote Cache

The local filesystem should remain the primary CAS.

Later, an optional remote cache could sit above it:

```text
remote CAS/cache
      ↓
local filesystem CAS
      ↓
projection
      ↓
consumer
```

Possible backends:

- S3
- R2
- MinIO
- shared filesystem
- CI cache service

Remote storage should accelerate population of the local CAS, not replace local execution paths.

---

## 13. Editors and Tool Discovery

Editors can become direct consumers of a project resolution.

Example project tools:

```text
clangd
rust-analyzer
ruff
prettier
typescript-language-server
```

Instead of requiring shell activation, an editor integration could ask ikk for resolved command paths.

A future machine-readable interface could expose:

```bash
ikk resolve --json
```

or similar.

This makes ikk useful to:

- Neovim
- Zed
- VS Code
- JetBrains tools
- LSP managers
- agents
- build systems

without requiring dedicated package-manager logic in each integration.

---

## 14. Reproducibility and Provenance

Because resolutions and store objects are content/version-addressed, future workflows can record:

```text
source commit
ikk resolution
target
command
task/agent input
```

This allows:

- reconstructing toolsets
- debugging different agent runs
- reproducing CI failures
- comparing toolchain changes
- rolling back quickly

A resolution hash can become a lightweight identity for a tool context.

---

## 15. Garbage Collection

CAS objects may be referenced by:

```text
global installs
project locks
persistent project views
saved CI/tool scopes
saved agent/task scopes
```

Ephemeral projections do not need to be permanent.

Future GC rule:

> Remove store objects only when no persistent reference requires them.

Avoid scanning the entire filesystem for project locks. Persistent project registrations or explicit roots can be introduced only if GC requires them.

---

## 16. What Not to Build Into the Core

Avoid early dedicated abstractions such as:

```text
DockerBackend
PodmanBackend
MicroVMBackend
NixShellMode
AgentManager
Scheduler
EditorManager
EnvironmentFramework
```

Prefer small reusable primitives.

Likewise, avoid prematurely adding:

- dependency solving
- centralized recipes
- large package manifests
- build farms
- custom scheduling
- container orchestration
- package-specific shims
- mandatory per-package binary declarations

Add these only if concrete use cases justify them.

---

## 17. Minimal Core Direction

The long-term core should remain close to:

```text
fetch
resolve
store
lock
materialize
execute
```

Possible internal concepts:

```text
Artifact
StoreObject
Target
Resolution
Projection
Lock
```

Potential APIs should remain consumer-neutral:

```text
resolve(requirements, target)
materialize(resolution, destination)
execute(projection, command)
```

Higher-level features are compositions:

```text
global install
= resolve + store + persistent PATH projection

project tooling
= resolve + store + project projection

shell
= project projection + interactive process

exec
= temporary resolution + projection + process

CI
= locked resolution + ephemeral projection + process

agent
= task requirements + target + ephemeral projection + process

container
= target-specific resolution + filesystem projection

microVM
= target-specific resolution + rootfs projection
```

---

## 18. Guiding Principles

### Store capabilities once

Expensive package contents should live once in CAS and be reused by many consumers.

### Compose access on demand

Do not equate availability with global installation.

### Persistence is optional

Global installs may be persistent. Project locks may be persistent. Agent tasks and scripts may be ephemeral.

### Target is explicit

Do not permanently couple resolution to the current host.

### Consumers stay outside the core

Shells, agents, editors, containers, VMs, CI, and schedulers should consume generic resolutions/projections.

### Prefer normal filesystem and process primitives

Symlinks, PATH, directories, processes, locks, and native OS facilities keep the system understandable.

### Add convenience without changing the model

Commands such as `ikk shell` or `ikk exec` should compose core primitives rather than introduce separate systems.

### Stay upstream-first

Where upstream publishes suitable release artifacts, consume them directly instead of introducing a second package-recipe ecosystem.

---

## 19. Strategic Opportunity

The broader opportunity is larger than a traditional package manager:

```text
traditional model

package
   ↓
install onto machine
```

versus:

```text
ikk model

requirements
    ↓
resolve tool resources
    ↓
store once
    ↓
project/materialize access where needed
    ↓
human, script, CI, agent, container, VM, editor, or future consumer
```

The long-term value is therefore not "another package manager".

It is a small, composable **tool provisioning substrate** built around:

```text
upstream release discovery
+
platform-aware resolution
+
local CAS
+
cheap projections
+
normal process execution
```

Global installation is simply the first consumer of that substrate.

---

## 20. Near-Term Priorities

Keep the current product focused.

1. Harden the existing global install path.
2. Keep the local filesystem CAS.
3. Make executable discovery deterministic and safe.
4. Make CAS insertion and upgrades transactional.
5. Preserve archive/path/symlink security boundaries.
6. Ensure target/platform is not unnecessarily coupled to the host.
7. Introduce project/ephemeral execution only through small reusable primitives.
8. Prefer `ikk exec` as the first generic non-global execution feature.
9. Add `ikk shell` only as a thin interactive convenience.
10. Let real use cases drive containers, agents, microVMs, remote caches, and other projections.

The architecture should evolve by composition, not by accumulating modes.
