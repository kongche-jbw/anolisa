# ANOLISA CLI Design Document

## Overview

`anolisa` is the helper of ANOLISA Agentic OS — the **Control Plane** of ANOLISA, targeted at human operators, responsible for installation, configuration, and health checks of ANOLISA's own capabilities and components.

```bash
$ anolisa list                       # what does ANOLISA give you on this machine?
$ anolisa enable token-optimization  # one-shot enable of a capability
$ anolisa status                     # is everything healthy?
```

### Target Users and Scenarios

`anolisa` targets ops engineers, application developers, and evaluators, covering the following typical scenarios:

| Scenario | Task |
|----------|------|
| New machine bring-up | Probe the environment, see which capabilities are available, enable on demand |
| Constrained-environment evaluation | On a container / nested VM / privilege-restricted machine, determine which capabilities are usable, which are not, and why |
| Ongoing operations | Monitor the health of enabled capabilities, adjust features on demand, upgrade component versions |
| Cross-distribution deployment | The same set of capabilities lands consistently on Alinux / Anolis / Ubuntu / Debian |

See [Common Tasks](#common-tasks) for detailed flows.

### Design Principle

Let users converse with **capabilities**, not with the implementation units (components). A capability is the functional dimension users want; a component is the software package that delivers it.

### Design Goals

- **Automated capability discovery**: `anolisa list` outputs an accurate available-capability list and unavailable-reason on any supported environment
- **Zero-judgment enablement**: `anolisa enable <capability>` performs the full flow of environment probing, dependency resolution, installation, configuration, and verification — no user pre-judgment required
- **Capability-first vocabulary**: the everyday path only sees capability-level vocabulary; error messages and repair advice never leak component / feature internal names
- **State traceability**: every enabled capability — its state, version, and feature configuration — is queryable, modifiable, and rollback-able
- **Cross-distribution consistency**: the same command behaves identically across distributions

### Boundaries

The following are explicitly **out of scope** for `anolisa`:

- **Does not perform system operations on behalf of Agents** (package install, service queries, snapshots, audit, etc.) — that is `cosh`'s (data-plane) responsibility; `anolisa` is only responsible for installing, configuring, and keeping `cosh` itself healthy
- **Does not manage user workloads** — Agent code written by users and its runtime configuration are out of scope
- **Does not manage application-layer configuration** — files like `/etc/nginx/nginx.conf` are out of scope
- **Does not provide monitoring/alerting or cluster orchestration** — cluster-orchestration software (K8s, Nomad, etc.) may in the future be installable as a capability via `anolisa enable`, but `anolisa` itself does not implement scheduling
- **No GUI for now** — CLI only; output simultaneously serves human reading and `--json` machine parsing

**`anolisa` is a convenience layer, not a mandatory path.** Once installed, components are independently usable — daily runs of `agentsight`, `tokenless`, etc. do not go through `anolisa`; installation can also bypass `anolisa` and use traditional methods like `dnf install`. `anolisa` provides the convenience of **unified orchestration, environment probing, and multi-component version alignment** — it does not take over how components themselves run or are used.

For the detailed boundary between `anolisa` and `cosh`, see [Appendix A](#appendix-a-anolisa-vs-cosh).

---

## Terminology

| Term | Meaning |
|------|---------|
| ANOLISA | OS product name (all caps) |
| `anolisa` | CLI binary name (lowercase), the command typed in the terminal |
| **Capability** | An Agent-facing capability from the customer's perspective (e.g. `token-optimization`). Also called "ANOLISA capability" to avoid confusion with Linux capabilities (e.g. `CAP_BPF`) |
| **Component** | The software unit that implements a capability (e.g. the `tokenless` component implements the `token-optimization` capability) |
| **Feature** | A sub-function within a component that can be toggled independently (e.g. the `rtk` feature of `tokenless`) |
| **Adapter** | The bridging package that integrates a component into an agent framework. Classified by framework origin into three kinds: `first-party` (e.g. `cosh`, in the same stack as anolisa, installed by default with the capability), `third-party` (e.g. `openclaw`, `hermes`, requires explicit user opt-in or post-discovery selection), `protocol` (e.g. `mcp`, follows an open protocol) |
| Substrate | osbase-layer foundation capabilities (e.g. sandbox, kernel modules) |

**Core abstraction hierarchy:** `capability → component(s) → feature(s) + environment requirements`

**Component layering:**

- **Runtime layer** — Tools that ANOLISA introduces into the OS, not native to the OS (e.g. `tokenless`, `agentsight`, `ws-ckpt`, `agent-sec-core`, `os-skills`, `cosh`)
- **Osbase layer** — Foundation components native to the OS (e.g. `kernel`, `sandbox` backends, `loongshield`). What `anolisa osbase install` installs is the **ANOLISA-optimized variant** — that is why a machine with an existing kernel may still need `anolisa osbase install kernel`: to swap in a kernel that includes ANOLISA-specific optimizations

---

## Capability Catalog

ANOLISA currently provides the following capabilities. Each capability is an independent "capability switch" from the customer's perspective, backed by one or more components plus optional features.

| Capability | What it provides | Implementation |
|---|---|---|
| `token-optimization` | LLM input/output token compression and rewriting | `tokenless` + features `rtk`, `toon`, `schema_compress` + adapters (`cosh`, `openclaw`, `hermes`) |
| `workspace-checkpoint` | Agent workspace snapshot and restore | `ws-ckpt` + storage backend (`btrfs` / `overlayfs`) + adapters (`cosh`, `openclaw`, `hermes`) |
| `agent-observability` | Agent behavior tracing and token attribution | `agentsight` + features `token_counting`, `ebpf_tracing` |
| `agent-security` | Security policy and audit for Agent operations | `agent-sec-core` + adapters (`cosh`, `openclaw`, `hermes`) |
| `agent-memory` | Agent persistent memory and context | `agent-memory` + adapters (`cosh`, `openclaw`, `mcp`) |
| `agent-skills` | OS operation skills bundle (for Agents to invoke) | `os-skills` |
| `agent-gateway` | Deterministic CLI gateway from Agent to OS | `cosh` |
| `sandbox` | Sandbox isolation for Agent workloads | `osbase sandbox` + backend feature (`kata` / `firecracker` / `landlock`) |
| `os-security` | OS kernel-level security hardening | `osbase security` + backend feature (currently the only backend: `loongshield`) |

Adding a new capability requires writing a capability manifest (see the [Capability Manifest](#capability-manifest) subsection).

**Capability naming principle:** A capability describes "the customer-facing functional dimension"; the name should focus on the capability itself and not reflect implementation choices. Implementation differences (backend, storage method, protocol, library version, etc.) are always expressed via features or backend selection — never as separate capabilities. Otherwise, naming proliferation like `sandbox-kata` / `sandbox-firecracker` / `agent-observability-ebpf` ("implementation slices masquerading as capabilities") will pollute the customer's mental model and break capability encapsulation. Even when a capability has only one implementation (e.g. `os-security` currently has only loongshield), name the capability after the capability itself and keep the implementation at the backend layer — that way adding new implementations later only requires adding a new backend, not renaming the capability.

---

## Common Tasks

### Task 1: New machine — see what's runnable / enable desired capabilities

```bash
$ anolisa env
Platform:     Physical (Alinux 4)
Kernel:       6.6.30 (BTF: yes, cgroups v2: yes, CAP_BPF: yes)
Storage:      btrfs available
Hypervisor:   KVM-capable
GPU:          NVIDIA A100 (driver 550.54)

$ anolisa list
CAPABILITY              STATUS       NOTE
token-optimization      available    -
workspace-checkpoint     available    btrfs backend
agent-observability     available    CAP_BPF available
agent-security          available    -
agent-skills            available    -
agent-gateway           available    -
sandbox                 available    backends: kata (KVM), landlock (kernel ≥ 5.13)
os-security             available    backend: loongshield

$ anolisa enable token-optimization workspace-checkpoint
  ✓ token-optimization        tokenless 0.3.2
  ✓ workspace-checkpoint       ws-ckpt 0.4.1 (btrfs backend)
  Run `anolisa status` to monitor ongoing health.
```

---

### Task 2: Evaluate what's usable in a constrained environment

Run inside a container:

```bash
$ anolisa env
Platform:     Container (runc via containerd, nested in KVM host)
Kernel:       6.6.30
Capabilities: CAP_NET_ADMIN, CAP_SYS_PTRACE (no CAP_BPF)
Storage:      overlayfs (btrfs: unavailable)

$ anolisa list
CAPABILITY              STATUS          NOTE
token-optimization      available       -
workspace-checkpoint     degraded        overlayfs backend (btrfs preferred)
agent-observability     unavailable     requires CAP_BPF
agent-security          available       -
sandbox                 unavailable     no backend usable (kata/fc need KVM; landlock needs kernel ≥ 5.13)

$ anolisa enable agent-observability
✗ Cannot enable agent-observability in this environment.
  Reason: requires CAP_BPF (current container lacks it)
  Advice: Run anolisa in a privileged container, or enable on the host.
```

---

### Task 3: Fine-tune sub-switches of an enabled capability

The security team wants to disable the HTTP dashboard of `agent-observability` while keeping token attribution:

```bash
$ anolisa status agent-observability
agent-observability — enabled (agentsight 0.2.0)
  features:
    ✓ token_counting    enabled
    ✓ server            enabled  (port 9090)
    ✓ ebpf_tracing      enabled

$ anolisa disable agent-observability --feature server
  ✓ Sub-feature 'server' disabled.
    Action: agentsight HTTP server on :9090 will stop on next restart.
```

---

### Task 4: Check overall health

```bash
$ anolisa status
CAPABILITY              STATE       DETAIL
token-optimization      ok          tokenless 0.3.2
workspace-checkpoint     degraded    ws-ckpt 0.4.1 (btrfs: loop fallback)
agent-observability     stopped     agentsight 0.2.0 (manually stopped)
agent-security          ok          agent-sec-core 1.2.0
agent-skills            ok          os-skills latest
agent-gateway           ok          cosh 2.1.0

$ anolisa doctor workspace-checkpoint
[workspace-checkpoint] Checking...
  ✓ Component: ws-ckpt 0.4.1 installed
  ✓ Service:   ws-ckpt.service (active, running)
  △ Storage:   using loop-device btrfs (degraded performance)
    Suggestion: mount a dedicated btrfs partition for production use
  ✓ Socket:    /run/ws-ckpt/ws-ckpt.sock (responsive, latency 0.3ms)
```

---

### Task 5: Build from source (development scenario)

By default `anolisa enable` installs from prebuilt packages. In development scenarios, build the underlying component from source:

```bash
$ anolisa runtime build tokenless
  Resolving build dependencies...
    ✓ rust 1.93.0
    ✓ just 1.40.0
  Building tokenless (release)...
    [1/3] Setting up RTK submodule...
    [2/3] cargo build --release (tokenless-cli)
    [3/3] cargo install toon
  ✓ Build complete → target/tokenless/

$ anolisa runtime build all --no-install
  Dependency graph: cosh → skills → (sec-core ∥ tokenless ∥ ws-ckpt)
  [parallel] Building sec-core, tokenless, ws-ckpt...
  ✓ All 5 components built → target/
```

This goes through Tier 2 `runtime build` (component-level) rather than Tier 1 `enable` (capability-level) — building is a component-level operation; once a component is built, it can be reused by multiple capabilities.

---

### Task 6: Tear down a capability

```bash
$ anolisa disable agent-observability --purge
  Confirm purge: this will remove all agentsight files and configs.
    Components to remove: agentsight
    Files:  ~/.local/bin/agentsight
            ~/.local/share/anolisa/libexec/agentsight/
    Config: ~/.config/anolisa/component.d/agentsight.toml
  Proceed? [y/N]
```

---

## Architecture

### System Architecture

```
┌────────────────────────────────────────────────────────────┐
│                    anolisa (CLI binary)                    │
│  ┌──────────────────────────────────────────────────────┐  │
│  │               CLI Layer (clap derive)                │  │
│  │   list · enable · disable · status · doctor · ...    │  │
│  └──────────────────────────┬───────────────────────────┘  │
│                             │                              │
│  ┌──────────────────────────▼───────────────────────────┐  │
│  │                 Capability Resolver                  │  │
│  │    capability → (components, features, env reqs)     │  │
│  └──────────────────────────┬───────────────────────────┘  │
│                             │                              │
│  ┌──────────────────────────▼───────────────────────────┐  │
│  │                 Orchestration Engine                 │  │
│  │   DependencyGraph · BuildPlan · TransactionRunner    │  │
│  └──────────────────────────┬───────────────────────────┘  │
│                             │                              │
│  ┌──────────────────────────▼───────────────────────────┐  │
│  │                    Core Services                     │  │
│  │  Registry·EnvProbe·FeatureStore·Subscription·State   │  │
│  └──────────────────────────┬───────────────────────────┘  │
│                             │                              │
│  ┌──────────────────────────▼───────────────────────────┐  │
│  │                 Platform Abstraction                 │  │
│  │   PkgMgr(dnf/apt)·FsLayout·Privilege·SystemdBridge   │  │
│  └──────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────┘
```

The middle layer **Capability Resolver** translates a capability (the customer-perspective noun) into an internal list of component + feature operations. Flow:

```
1. CLI receives `anolisa enable token-optimization`
2. Resolver loads the capability manifest → resolves component=[tokenless], features=[rtk, toon, ...]
3. Resolver invokes EnvProbe to verify the requires_env declared in the capability manifest
4. If unsatisfied → fail and assemble repair advice for return
5. If satisfied → submit execution plan to the Orchestration Engine: runtime install tokenless + configure features + verify
6. Aggregate results per capability and return to the CLI Layer
```

Tier 2 commands (`runtime install`, `osbase sandbox install`, etc.) skip the Resolver and go directly to the Orchestration Engine — because the caller has already explicitly chosen the component / target and no capability-level translation is needed.

### Command Surface

`anolisa`'s command surface is two-tiered:

- **Tier 1 — Capability commands**: verbs operating on capabilities, for everyday use. Sub-item configuration (features, etc.) is performed via engineering parameters (`--feature`, etc.) within the capability verb
- **Tier 2 — Management surfaces**: independent, orthogonal subsystem-management functions (subscription, adapter, self, runtime, osbase). Each surface uses vocabulary appropriate to what it manages, and is an explicit external promise on equal footing with Tier 1

Tier 1 internally goes through the Capability Resolver to translate to underlying runtime / osbase operations; Tier 2 is the explicit entry point for those underlying operations, used when the caller wants to directly target a specific component or target. The vocabulary and error boundary between the two tiers is enforced by [Layer Discipline](#layer-discipline).

#### `anolisa --help`

```text
anolisa — ANOLISA Agentic OS helper

Usage:
  anolisa <CAPABILITY-VERB> [ARGS] [OPTIONS]
  anolisa <SURFACE> <SUBCOMMAND> [ARGS] [OPTIONS]

CAPABILITY COMMANDS — Tier 1, capability-vocabulary verbs for everyday use.

  list                     List capabilities and their availability / enable status
                             [--available]  [--enabled]  [--json]

  enable <CAPABILITY>...   Enable one or more capabilities
                             [--feature <NAME>]
                             [--with-adapter <FRAMEWORKS|auto>]    # FRAMEWORKS: comma-separated;
                                                                   # `auto` = install all detected
                             [--from-source]  [--dry-run]

  disable <CAPABILITY>     Disable a capability or one of its features
                             [--feature <NAME>]  [--purge]

  status [CAPABILITY]      Show capability health (aggregate or per-capability)
                             [--json]

  doctor [CAPABILITY]      Diagnose capability issues
                             [--fix]

  logs <CAPABILITY>        Tail / show service logs for a capability (if it has a service)
                             [--follow]  [--since <DURATION>]  [--lines <N>]

  restart <CAPABILITY>     Restart the capability's underlying service (no reinstall)

  env                      Show environment detection results
                             [--json]  [--verbose]

  info                     One-shot summary: anolisa version + enabled capabilities
                           with versions + installed components + osbase status
                             [--json]

  update [TARGET]          Update components behind a capability (TARGET: capability or `all`)
                             [--dry-run]

MANAGEMENT SURFACES — Tier 2, independent subsystem surfaces.

  subscription             Manage ANOLISA subscription
    register                 Register a subscription           [--org]  [--key]  [--server]
    unregister               Unregister                        [--force]
    status                   Show subscription status          [--json]
    refresh                  Refresh subscription token

  adapter                  Manage agent-framework adapters (cosh / openclaw / hermes / mcp / ...)
    list                     List installed / available adapters [--json]
    install <COMP> <FW>      Install <component>'s adapter for <framework>
    remove <COMP> <FW>       Remove an adapter
    scan                     Probe machine for installed agent frameworks (discovery only,
                               no install) [--json]

  self                     Manage anolisa CLI itself
    update                   Update the anolisa CLI binary
    adopt                    Scan & register pre-existing components (build-all.sh migration) [--scan] [--confirm]
    completions <SHELL>      Generate shell completion script

  runtime                  Manage runtime-layer components directly
    install <COMP|all>       Install component(s)              [--from-source | --from-rpm | --version <V>]
    remove <COMP>            Remove a component                [--purge]
    update <COMP|all>        Update component(s)
    build <COMP|all>         Build from source                 [--release | --debug | --no-install]
    list                     List installed / available comps  [--available]  [--json]
    status [COMP]            Show component status             [--json]

  osbase                   Manage OS base layer (kernel → sandbox → security)
    kernel install           Install kernel modules / patches  [--dry-run]
    kernel remove            Remove kernel modules / patches
    kernel status            Show kernel layer status
    sandbox install <T>      Install sandbox runtime           [--dry-run]
    sandbox remove <T>       Remove sandbox runtime
    sandbox list             List sandbox targets              [--available]
    sandbox status [T]       Show sandbox runtime status
    security install <T>     Install security module           [--dry-run]
    security remove <T>      Remove security module
    security status [T]      Show security module status

    Targets:  sandbox  = container | kata | firecracker | vm | landlock
              security = loongshield | seccomp-profiles

GLOBAL OPTIONS:
      --install-mode <user|system>     Install scope [default: user]
      --prefix <PATH>                  Install prefix override
  -v, --verbose
  -q, --quiet
      --json                           Machine-readable output
      --dry-run                        Print plan without executing
      --no-color
  -h, --help                           Print help
  -V, --version                        Print version
```

### Layer Discipline

Tier 1 (capability vocabulary) and each Tier 2 surface (subscription / adapter / self vocabulary, runtime's component vocabulary, osbase's target vocabulary) must have **structural isolation** between them, enforced via code organization rather than manual care.

The vocabulary boundary is **drawn by surface, not by tier**: each surface uses, and only uses, vocabulary appropriate to its managed object.

**1. Tier 1 is the controlled outward semantic layer**
Tier 1 commands' error messages, status output, and repair advice must not contain component names, feature internal names, raw config keys, or osbase target names. From the customer's perspective at Tier 1, only capability vocabulary is visible.

**2. Tier 2 is the explicit subsystem entry point**
Commands like `runtime install tokenless` and `osbase sandbox install kata` are legitimate paths for "directly operating on a concrete object" — the caller has already expressed intent for the internal object, and the surface receives it in the corresponding object vocabulary.

**Code-level enforcement:**

- Tier 1 command paths **must go through the Capability Resolver**; Tier 1 handlers may not import component-level / osbase-level types directly
- Each Tier 2 surface handler **bypasses the Resolver** and goes directly to the Orchestration Engine to invoke the corresponding executor
- Error types are partitioned by surface: Tier 2 runtime throws `ComponentError`, osbase throws `OsbaseError`, subscription throws `SubscriptionError`; before bubbling up to the Tier 1 outlet, they **must** be translated by the Resolver into a `CapabilityError` (carrying the capability name plus customer-language repair advice)
- Output formatters are separated: `tier1::format_capability_*()` does not hold any component / target / feature fields; each Tier 2 surface uses its own formatter

**Negative failure mode:**
If isolation is not enforced at the code level, Tier 1 will exhibit vocabulary leakage (component names appearing in error messages, capability vocabulary semantics drifting); subsequently the Tier 2 surface becomes the only reliable entry point, with users forced to use `runtime install` to perform what should have been done by `enable <capability>`. This is the direct manifestation of failed modular design.

### Capability Manifest

Each capability has a manifest describing how it maps to component(s) + feature(s), and its environment requirements.

```toml
[capability]
name = "token-optimization"
description = "LLM input/output token compression and rewriting"

[implementation]
components = ["tokenless"]
features.tokenless = ["rtk", "toon", "schema_compress"]

[requires_env]
# Capability-level env requirement = union of underlying components' requirements + extra constraints
os = "linux"
arch = ["x86_64", "aarch64"]
```

A capability's "degraded" and "unavailable" states are derived by the Resolver by comparing the capability manifest against the current `EnvFacts`.

### Component Manifest

Manifest filename convention: `component.toml`, one per component, describing its identity, build method, install location, environment requirements, dependencies, available features, and adapters.

```toml
[component]
name = "tokenless"
version = "0.3.2"
layer = "runtime"                    # osbase | runtime | encapsulation
domain = "cost"                      # tools | state | cost | security | observability
description = "LLM token optimization toolkit"

[build]
system = "cargo"                     # cargo | npm | make | static
targets = ["tokenless-cli"]
toolchain = { rust = ">=1.91.0", just = ">=1.0" }

[[install.files]]
source = "target/release/tokenless"
dest = "{bindir}/tokenless"
mode = "0755"

[environment]
requires_os = "linux"
requires_arch = ["x86_64", "aarch64"]
requires_kernel = ">=5.4"
incompatible_env = []

[dependencies]
build = ["rust>=1.91", "just>=1.0"]
runtime = []
components = []

[[features]]
name = "rtk"
label = "RTK command rewriting"
default = true
requires_env = {}

# Adapter section: declares which frameworks this component can integrate with.
# Installed under the unified path share/anolisa/adapters/<component>/<framework>/.
# kind: first-party / third-party / protocol — drives default install behavior at enable time.
[[adapters]]
framework = "cosh"
kind = "first-party"                 # installed by default with the capability
source = "target/release/cosh-ext/"
dest = "{datadir}/anolisa/adapters/{component}/cosh/"

[[adapters]]
framework = "openclaw"
kind = "third-party"                 # not installed by default; opt-in via --with-adapter or post-scan
source = "target/release/openclaw-plugin/"
dest = "{datadir}/anolisa/adapters/{component}/openclaw/"
detect = { binary = "openclaw" }     # detection hint: `openclaw` on PATH

[[adapters]]
framework = "hermes"
kind = "third-party"
source = "target/release/hermes-plugin/"
dest = "{datadir}/anolisa/adapters/{component}/hermes/"
detect = { binary = "hermes", paths = ["/opt/hermes"] }
```

> **Unified adapter model**: cosh's extension is no longer a separate "extension" concept — it is an adapter just like OpenClaw / Hermes / MCP, differing only in `kind`. Install path is unified at `share/anolisa/adapters/<component>/<framework>/`.

> **Where detection rules live**: the `detect` field is an auxiliary annotation; the authoritative framework-detection rules are **centrally maintained in anolisa's built-in framework-probe library** (see [Environment Detection](#environment-detection)), independent of per-component manifests — that way a new framework only requires upgrading anolisa, not editing every component manifest.

> **Path placeholders**: the resolution rules for `{bindir}` / `{libexecdir}` / `{datadir}` etc. used in `dest` are documented in [Filesystem Layout](#filesystem-layout).

### Filesystem Layout

`anolisa` install paths are subject to a **hard constraint**:

- **system-mode** strictly follows [FHS 3.0](https://refspecs.linuxfoundation.org/FHS_3.0/fhs/index.html)
- **user-mode** strictly follows the [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html)

#### Path Mapping

| Purpose | system-mode (FHS) | user-mode (XDG) |
|---------|-------------------|-----------------|
| Main binary | `/usr/local/bin/` | `~/.local/bin/` (XDG_BIN_HOME, de-facto) |
| Helper / libexec | `/usr/local/libexec/anolisa/` | `~/.local/share/anolisa/libexec/` |
| Shared data (skills / adapters) | `/usr/local/share/anolisa/` | `$XDG_DATA_HOME` (default `~/.local/share`) `/anolisa/` |
| Configuration | `/etc/anolisa/` | `$XDG_CONFIG_HOME` (default `~/.config`) `/anolisa/` |
| State files (installed.toml etc.) | `/var/lib/anolisa/` | `$XDG_STATE_HOME` (default `~/.local/state`) `/anolisa/` |
| Logs / audit | `/var/log/anolisa/` | `$XDG_STATE_HOME/anolisa/` |
| systemd units | `/etc/systemd/system/` | `~/.config/systemd/user/` |
| Runtime (socket / pid) | `/run/anolisa/` | `$XDG_RUNTIME_DIR/anolisa/` |
| Cache (probes / build artifacts) | `/var/cache/anolisa/` | `$XDG_CACHE_HOME` (default `~/.cache`) `/anolisa/` |

#### Component Manifest Placeholder Resolution

Component manifests use placeholders (`{bindir}`, etc.) to declare install destinations; `anolisa-platform`'s `FsLayout` resolves them at runtime per mode:

| Placeholder | system-mode | user-mode |
|-------------|-------------|-----------|
| `{bindir}` | `/usr/local/bin` | `~/.local/bin` |
| `{libexecdir}` | `/usr/local/libexec/anolisa` | `~/.local/share/anolisa/libexec` |
| `{datadir}` | `/usr/local/share/anolisa` | `~/.local/share/anolisa` |
| `{sysconfdir}` | `/etc/anolisa` | `~/.config/anolisa` |
| `{statedir}` | `/var/lib/anolisa` | `~/.local/state/anolisa` |
| `{logdir}` | `/var/log/anolisa` | `~/.local/state/anolisa` |
| `{runtimedir}` | `/run/anolisa` | `$XDG_RUNTIME_DIR/anolisa` |
| `{cachedir}` | `/var/cache/anolisa` | `~/.cache/anolisa` |

#### Hard Constraints

- Component manifests **must not hardcode absolute paths** — use placeholders. anolisa-core lints during manifest load; violations are rejected at registration
- Any component crossing the constrained directories (e.g. user-mode trying to write `/etc`, system-mode writing into the user's HOME) is rejected with an error
- The global `--prefix <PATH>` flag can redirect the system-mode prefix as a whole (default `/usr/local`), used for distribution scenarios that package to non-default prefixes like `/opt/anolisa/`; user-mode does not support `--prefix` and instead respects XDG environment variables
- Path resolution is consistent across distributions (Alinux / Anolis / Ubuntu / Debian) — differences in systemd unit directory, polkit rule directory, etc. are abstracted by anolisa-platform

### Feature Flag System

#### Storage: `$CONFIG_DIR/features.toml`

```toml
[component.tokenless]
enabled = true

[component.tokenless.features]
rtk = true
toon = true
schema_compress = true

[component.ws-ckpt]
enabled = true

[component.ws-ckpt.features]
btrfs_loop = true
overlayfs = false
```

#### Merge Priority

compile-time defaults < `/etc/anolisa/features.toml` < `~/.config/anolisa/features.toml` < env var `ANOLISA_FEATURE_*`

#### Constraint Declaration (in component manifest)

```toml
[[features]]
name = "btrfs_loop"
default = true
requires_env = { kernel_version = ">=5.4", filesystem = "btrfs_available" }
conflicts_with = ["overlayfs"]
```

### Environment Detection

Fine-grained identification of the host's platform, virtualization layer, kernel features, filesystem, Linux capabilities, etc., serving as the basis for capability-compatibility judgment and feature gating.

#### Probe List

| Probe | What it detects | Key methods |
|-------|-----------------|-------------|
| platform | Physical / VM / Container | DMI, systemd-detect-virt, cgroup |
| hypervisor | KVM / Xen / HyperV / VMware | CPUID 0x40000000, DMI product_name |
| container | runc / gVisor / Kata / Firecracker | /proc/1/cgroup, /.dockerenv, kernel cmdline |
| nesting | Nesting layers | cross-validation across multiple signals |
| gpu | NVIDIA / AMD / Intel + driver | PCI, /dev/nvidia* |
| tee | TDX / SEV / SGX | CPUID, /dev/tdx_guest |
| kernel | version, BTF, cgroups v2, landlock ABI | uname, sysfs |
| distro | ID, version, pkg_base (rpm/deb) | /etc/os-release |
| filesystem | btrfs / overlayfs available | mount, modprobe |
| capabilities | CAP_BPF, CAP_SYS_ADMIN | /proc/self/status |
| frameworks | Installed agent frameworks: cosh / openclaw / hermes / mcp / ... | PATH lookup, conventional paths, client config-file signatures |

#### Gating

Each component manifest declares `requires_env`; a capability manifest may layer additional constraints on top. Before install/enable, the Resolver auto-validates and, on failure, emits the reason plus repair advice.

#### Adapter Framework Probe

The `frameworks` probe shares the same plumbing as other probes, but its target is agent frameworks rather than platform features. Its output feeds two entry points:

- **`anolisa adapter scan`** — read-only output of the framework list installed on the current machine (human / `--json`); installs nothing
- **`anolisa enable <cap> --with-adapter=auto`** — probe + auto-install adapters for all detected frameworks (non-interactive)

An explicit list (`--with-adapter=cosh,openclaw`) skips auto-detection and installs directly; a framework specified but not detected fails with repair advice (e.g. `openclaw not detected; install openclaw or remove from --with-adapter list`).

Framework probe rules are centrally maintained in `anolisa-env/src/probes/frameworks.rs` — adding a new framework only requires upgrading anolisa, not editing component manifests.

---

## Distribution & Delivery

### Delivery Form for `anolisa` Itself

| Channel | Target users | Form | Notes |
|---------|--------------|------|-------|
| GitHub Releases | Open-source community / evaluators | Static binary (x86_64 + aarch64) | Single-file download, zero dependencies |
| Alinux 4 YUM repo | Alibaba Cloud production | RPM | Reuses existing ANOLISA yum repo, `dnf install anolisa` |
| Anolis OS / OpenAnolis | Community-distro users | RPM | Same as above, shared repo |
| Ubuntu / Debian | External developers | DEB | Separate apt repo or PPA |
| Install script | Quick start | `curl -sSf https://... \| sh` | rustup-style; detects platform and downloads matching binary |
| Container image | CI / testing / evaluation | `registry.cn-hangzhou.aliyuncs.com/anolisa/anolisa:latest` | For pipeline integration or root-less evaluation |

### Component Delivery Form

Once `anolisa` is installed, components behind capabilities can be obtained two ways:

| Mode | Trigger command | Use case |
|------|-----------------|----------|
| Prebuilt package | `anolisa enable <capability>` (default) | Production, fast deployment |
| Build from source | `anolisa enable <capability> --from-source` or `anolisa runtime build <component>` | Development, custom builds, unreleased versions |

Both modes share the same manifest validation, environment detection, and feature-flag configuration flow.

### Bootstrap

`anolisa` itself is obtained via system package manager or install script (no self-dependency). After installation, it manages the lifecycle of all capabilities.

```
system package manager (dnf/apt)  ─────→  anolisa binary
                                              │
                                              ▼
                                       anolisa enable <capability...>
                                              │
                                              ▼
                                component installed + features configured + verified
```

### Versioning Policy

- `anolisa`'s own version is independent of individual component versions
- `anolisa self update` updates the CLI itself
- `anolisa update <capability>` updates the components behind a capability
- The manifest declares `anolisa_min_version` to constrain compatibility

---

## Migration Path

### Adopting Components Installed via `build-all.sh`

Existing ANOLISA users may already have components installed via `build-all.sh`. After installing `anolisa`, **no reinstall is required** — a scan-and-adopt flow is provided:

```bash
$ anolisa self adopt --scan
Scanning for existing ANOLISA components...
  ✓ tokenless 0.3.0      found at ~/.local/bin/tokenless (installed via build-all.sh)
  ✓ ws-ckpt 0.4.0        found at ~/.local/bin/ws-ckpt
  ✓ agentsight 0.1.8     found at ~/.local/bin/agentsight (older than registry: 0.2.0)

Adopt these into anolisa management? Run with --confirm.

$ anolisa self adopt --scan --confirm
  ✓ 3 components registered in ~/.local/state/anolisa/installed.toml
  ✓ Feature flags reset to defaults (override via `anolisa enable <cap> --feature ...`)

Recommended next step: `anolisa update all` to align versions with the registry.
```

After adoption, all subsequent operations (upgrade, uninstall, status query, feature adjustment) go through `anolisa`; `build-all.sh` is no longer needed for daily use.

### Fate of `build-all.sh`

- **Retained** as a low-level build tool for development scenarios — used by component developers and CI. `anolisa enable --from-source` reuses its logic internally.
- **Deprecated** as the customer install entry point. README / documentation "customer install" sections are updated to recommend the `anolisa enable` path.
- **Transition period**: both run in parallel for at least one release cycle, giving existing users adequate time to migrate.

### Out of Migration Scope

- Non-ANOLISA software already deployed (user-installed tools) — `anolisa` neither adopts nor affects these
- User-modified component versions (out of sync with the registry) — `adopt` flags these as "out-of-registry" and subsequent `anolisa update` will not overwrite, unless `--force` is explicitly passed

---

## Security Considerations

### Privilege Model

- **`--install-mode user`** (default): installs to `~/.local/` per XDG, no sudo required, single-user scope
- **`--install-mode system`**: installs to the `/usr/local/` prefix per FHS (redirectable via `--prefix`), elevates via polkit, machine-wide scope
- Full path conventions: see [Filesystem Layout](#filesystem-layout)
- `anolisa` itself does not carry a setuid bit; privilege-elevation points are concentrated in `anolisa-platform/src/privilege.rs` for single-point auditability
- Only one `anolisa` process at a time may write the state file (flock); in multi-user setups, each user's user-mode state is mutually isolated

### Manifest Trust Chain

- **Built-in manifests** ship with the `anolisa` binary, are protected by ANOLISA release signatures, and cannot be replaced at runtime
- **Downloaded prebuilt component packages** are verified via SHA256 + publisher signature; verification failure rejects the install
- **Build from source** (`--from-source`): the build runs in a restricted environment (minimal PATH, network access limited to git submodules and dependency mirrors); users may pass `--no-sandbox` to disable, at their own risk
- **Third-party / user-supplied manifests** are not supported. If opened up later, an independent root of trust plus an explicit confirmation flow will be required.

### Audit Trail

All `anolisa enable / disable / update`, `anolisa runtime install`, and `anolisa osbase * install` operations write to an audit log:

- user-mode: `~/.local/state/anolisa/audit.log`
- system-mode: `/var/log/anolisa/audit.log`

Each record contains timestamp, operation type, object (capability / component / target), caller, and exit status. Credentials (key / token) involved in `anolisa subscription` are stored separately in `~/.config/anolisa/credentials.toml` (mode `0600`), and never written to the audit in plaintext.

### Threat Model and Mitigations

| Threat | Mitigation |
|--------|------------|
| Tampered prebuilt package (MITM) | Triple validation: HTTPS + SHA256 + release signature |
| Malicious component manifest | Only built-in manifests are trusted; extending to third-party manifests requires an independent trust chain |
| Local privilege escalation (non-sudo user enabling machine-wide capabilities) | user-mode is isolated by default; system-mode requires explicit polkit authorization |
| Build-process supply-chain attack (malicious source / dependencies) | Build is sandboxed (restricted PATH, network allowlist) |
| Credential leakage | `credentials.toml` strict permissions; not in audit, not in logs |
| Concurrent state-file writes corrupting state | flock mutual exclusion; see [TBD](#tbd) for details |

---

## Implementation

### Project Structure

```
src/anolisa/
├── Cargo.toml                       # Workspace root
├── crates/
│   ├── anolisa-cli/                 # CLI binary
│   │   └── src/
│   │       ├── main.rs
│   │       └── commands/
│   │           ├── tier1/           # mod.rs + one file per verb
│   │           │   ├── mod.rs
│   │           │   ├── list.rs
│   │           │   └── ...          # enable, disable, status, doctor, logs, restart, env, info, update
│   │           ├── subscription.rs
│   │           ├── adapter.rs       # + self_.rs, runtime.rs, osbase.rs
│   │           └── ...
│   │
│   ├── anolisa-core/                # business-logic lib
│   │   ├── src/
│   │   │   ├── capability.rs        # CapabilityManifest + Resolver
│   │   │   ├── component.rs         # Component trait + meta
│   │   │   ├── manifest.rs          # component TOML parsing (incl. [[adapters]])
│   │   │   ├── registry.rs          # manifest loading + registration
│   │   │   ├── dependency.rs        # DAG dependency resolution
│   │   │   ├── transaction.rs       # atomic install/rollback
│   │   │   ├── feature_flags.rs     # feature storage + merging
│   │   │   ├── subscription.rs
│   │   │   └── state.rs             # installed.toml tracking
│   │   └── tests/
│   │       └── capability_manifest.rs   # smoke test: all 9 capability manifests parse
│   │
│   ├── anolisa-env/                 # environment-detection lib
│   │   └── src/
│   │       ├── lib.rs               # EnvFacts + DetectedFramework structs
│   │       ├── gate.rs              # requirement-evaluation engine
│   │       ├── cache.rs             # probe-result cache
│   │       └── probes/
│   │           ├── mod.rs
│   │           ├── platform.rs
│   │           └── ...              # kernel, distro, frameworks
│   │
│   ├── anolisa-build/               # build-orchestration lib
│   │   └── src/
│   │       ├── lib.rs
│   │       └── backends/            # cargo / npm / static (cargo only at skeleton)
│   │
│   └── anolisa-platform/            # platform-abstraction lib
│       └── src/
│           ├── package_manager.rs   # dnf/apt/zypper
│           ├── fs_layout.rs         # placeholder resolution
│           ├── privilege.rs         # sudo/polkit
│           └── systemd.rs           # service management
│
└── manifests/                       # built-in manifests
    ├── capabilities/                # Capability manifests (customer view)
    │   ├── token-optimization.toml
    │   ├── workspace-checkpoint.toml
    │   └── ...                      # 9 total: see Capability Catalog
    ├── runtime/                     # Component manifests (runtime layer)
    │   ├── cosh.toml
    │   ├── tokenless.toml
    │   └── ...                      # 7 total
    └── osbase/                      # Component manifests (osbase layer)
        ├── kernel.toml
        ├── sandbox-kata.toml
        └── ...                      # 6 total
```

### Skeleton Phases

The skeleton phase (without real business logic) lands in this order:

1. **Create workspace** — `src/anolisa/` + 5 crates
2. **CLI command tree** — clap derive defines all Tier 1 + Tier 2 commands; handlers return placeholders
3. **Core traits** — `Capability`, `Component`, `FeatureDef` definitions
4. **Manifest parsing** — TOML struct definitions for both capability and component, plus serde parsing
5. **Capability Resolver** — capability → component+feature translation, including env gate
6. **EnvFacts struct** — environment-detection data structure + basic platform probe
7. **FsLayout** — user/system path resolution
8. **Built-in manifests** — write manifest files for existing components and the 9 built-in capabilities

### Test Strategy

| Layer | Scope | Tooling | CI trigger |
|-------|-------|---------|------------|
| **Unit tests** | Internal logic of each crate: manifest parsing, resolver translation, env gate, TOML serialization, error types | `cargo test` | every PR |
| **CLI integration tests** | Argument parsing, exit codes, `--json` output format, error messages for each subcommand | `assert_cmd` + `predicates` | every PR |
| **Env-detection integration tests** | Verify probe output in mocked environments (fake `/proc`, fake DMI) | self-built fixtures | every PR |
| **Cross-distro e2e** | Run `enable → status → disable` for the 9 capabilities inside Alinux 4 / Anolis 8 / Ubuntu 22 / Debian 12 containers | GitHub Actions matrix + Docker | nightly + before release |
| **Bare-metal integration tests** | Validate critical capabilities (`sandbox` backends, `os-security`) on physical hosts + nested KVM | self-managed test pool | weekly + before release |
| **Snapshot tests** | CLI output formats (human-readable + `--json`) — guard against accidental regressions | `insta` snapshots | every PR |
| **Coverage gate** | Core logic (resolver, transaction, probe) ≥ 80% line coverage | `cargo tarpaulin` | every PR; below-threshold blocks merge |

**Critical-path required cases:**

- Full matrix of 9 capabilities × {`enable`, `status`, `disable`} × {`--install-mode user`, `--install-mode system`}
- `enable` failure scenarios: env not satisfied, network failure, dependency conflict — error-message snapshots verified
- Concurrency: two `anolisa` processes writing the state file simultaneously, validating flock behavior
- Upgrade chain: install v1 → upgrade to v2 → roll back to v1 → uninstall, with state-file consistency at each step
- Vocabulary-isolation lint: scan all user-visible Tier 1 output for component / feature / target internal names — must be zero

**Release Acceptance Gate**

The following are blocking gates for release — failing any of them blocks shipping:

| Dimension | Gate |
|-----------|------|
| Cross-distro coverage | All 9 capabilities pass `enable → status → disable` e2e on Alinux 4 / Anolis 8 / Ubuntu 22 / Debian 12 |
| Time-to-first-capability | When the environment is satisfied, `dnf install anolisa` to first capability available ≤ 5 minutes, zero manual intervention |
| Vocabulary isolation | All user-visible Tier 1 output (errors, status, repair advice) passes lint — zero leakage of component / feature internal names |
| Reversibility | Any `enable`-d capability can be fully reverted by `disable`; after `enable → disable` the machine state matches the pre-enable baseline |

### Verification

```bash
cd src/anolisa
cargo build                          # compiles
cargo run -- --help                  # shows the full two-tier command surface
cargo run -- list                    # lists all capabilities + status
cargo run -- env                     # outputs placeholder / basic detection
cargo run -- enable token-optimization --dry-run  # shows resolver translation result
cargo run -- runtime list --available             # Tier 2 runtime surface: shows components
cargo test                           # unit tests pass
```

### Key Source Locations

| Path | Purpose |
|------|---------|
| `src/anolisa/` | CLI implementation root |

---

## TBD

The following items are deferred to later iterations:

- **install-mode isolation**: when both `--install-mode user` and `--install-mode system` exist on the same machine, how are the state files distinguished? Upgrade path?
- **Rollback boundary**: `TransactionRunner`'s rollback strategy in the half-success state where files are on disk but the service failed to start?
- **Manifest schema evolution**: add a `manifest_version` field to `capability.toml` / `component.toml`; compatibility matrix for new `anolisa` reading old manifests?
- **Concurrency mutex**: state-file write mutex when multiple `anolisa` processes run concurrently on the same machine (lockfile / flock / advisory only?)
- **Capability composition**: should capabilities support `requires` / `conflicts` between each other (e.g. `agent-observability-full = agent-observability + agent-security`)? Recommendation in the short term: don't — keep capability ↔ component as 1:1 or 1:N

---

## Appendix A: `anolisa` vs `cosh`

### Dimension Comparison

| Dimension | `anolisa` | `cosh` |
|-----------|-----------|--------|
| Audience | Humans (ops / developers) | Agents (LLM Tool Use) |
| Function | Manages the ANOLISA stack itself | Performs OS operations on behalf of Agents |
| Input | Interactive CLI (progress bars, prompts, tables) | Structured JSON / CLI args |
| Output | Human-readable (diagnostic advice, colored status) | `CoshResponse<T>` JSON (machine-parseable) |
| Operates on | ANOLISA's own capabilities and components | Host system resources |
| OS analogy | `apt` / `dnf` (package manager) | `systemctl` / `ip` (system-operation commands) |

**One sentence**: `anolisa` manages "what ANOLISA looks like"; `cosh` performs "what the Agent wants to do to this machine".

### Boundary Across `cosh`'s Phased Evolution

`cosh` evolves through phases (NLP Shell → Deterministic CLI Gateway → IPC Bus). For all phases, the boundary is consistent: `cosh` is the data-plane primitive (executes pkg/svc/checkpoint/audit on behalf of the Agent), and `anolisa` manages `cosh`'s own installation, version, and features. When `cosh` reaches the IPC Bus phase, `anolisa` may **reuse** `cosh`'s cross-distro routing as its underlying execution channel — `anolisa` provides capability-level orchestration (env check → dependency resolution → install → verify), `cosh` provides the execution primitive (cross-distro routing). This is **layered reuse**, not overlap.
