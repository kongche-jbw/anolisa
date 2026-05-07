# Agentic OS for OpenClaw Solution

## Overview

Agentic OS for OpenClaw is an integrated solution that connects existing Agentic OS capabilities to OpenClaw. The goal is to provide one installation path, one operational model, and one troubleshooting surface for capabilities that were originally developed as independent components.

The solution is delivered through:

- a one-click installer at `extension/openclaw/agenticos2openclaw.sh`
- an OpenClaw-facing Skill at `extension/openclaw/SKILL.md`
- this architecture and user-facing solution document

## Component Mapping

| Component | Capability | OpenClaw Integration |
|---|---|---|
| cosh | one-click installation and shell integration | provides the install entry pattern and can install OpenClaw itself |
| SKILL | reusable Skill library | migrates Agentic OS Skills into the OpenClaw Skill directory |
| sec-core | sandbox, asset verification, security Skills | attaches runtime security and security Skills to OpenClaw |
| Sight | diagnostics and observability | diagnoses OpenClaw agent behavior, token usage, and runtime events |
| osbase | system optimization | applies memory-side optimization for common small OpenClaw instances |
| runtime | runtime optimization | provides ws-ckpt, skillfs, skvm, and other OpenClaw-oriented runtime improvements |
| token-less | token reduction | integrates OpenClaw plugin paths and runtime hooks for context compression and output filtering |

## Architecture

The solution uses a layered architecture:

1. OpenClaw Agent Gateway is the base runtime.
2. The installer validates prerequisites and installs OpenClaw when needed.
3. System modules such as osbase and Sight are checked or installed through RPMs.
4. Runtime modules such as skillfs, ws-ckpt, and skvm are installed and exposed to OpenClaw as Skills or services.
5. Security modules such as sec-core install their runtime components, plugin hooks, and Skills.
6. Token optimization modules such as token-less register OpenClaw plugin paths and hooks.
7. The state file records module results for status, retry, and rollback.

Default paths:

| Path | Purpose |
|---|---|
| `/usr/share/anolisa` | Agentic OS component root |
| `/usr/share/anolisa/skills` | Agentic OS Skill source directory |
| `/usr/share/anolisa/runtime/skills` | runtime Skill source directory |
| `~/.openclaw/openclaw.json` | OpenClaw configuration |
| `~/.openclaw/skills` | OpenClaw Skill target directory |
| `~/.openclaw/agenticos-state.json` | installer state |
| `~/.openclaw/backups` | OpenClaw config backups |

## Deployment Topology

Recommended single-node topology:

```text
OpenClaw host
├── OpenClaw Agent Gateway
├── Agentic OS installer
├── OpenClaw Skills
│   ├── migrated Agentic OS Skills
│   ├── skillfs
│   ├── ws-ckpt
│   └── sec-core Skills
├── system services
│   ├── agentsight
│   └── skvm-bridged
└── OpenClaw plugins
    ├── sec-core integration
    └── token-less integration
```

Recommended resource profile:

| Scenario | CPU | Memory | Notes |
|---|---:|---:|---|
| minimal validation | 2 vCPU | 2 GB | use `--mode sec-core` or targeted modules |
| recommended production baseline | 2-4 vCPU | 4 GB | use `--mode recommended` |
| full diagnostics | 4+ vCPU | 8 GB | use `--mode all` with Sight enabled |

Small instances benefit from osbase memory tuning. If osbase is not present, the installer treats it as non-fatal outside the expected Anolis SWAS environment.

## Installation

Recommended remote install:

```bash
curl -fsSL https://raw.githubusercontent.com/alibaba/anolisa/main/extension/openclaw/agenticos2openclaw.sh | bash -s -- --mode recommended
```

Full install:

```bash
curl -fsSL https://raw.githubusercontent.com/alibaba/anolisa/main/extension/openclaw/agenticos2openclaw.sh | bash -s -- --mode all
```

Local development install:

```bash
bash extension/openclaw/agenticos2openclaw.sh --mode recommended
```

Preview the plan:

```bash
bash extension/openclaw/agenticos2openclaw.sh --mode recommended --dry-run
```

Use a custom Agentic OS component root:

```bash
bash extension/openclaw/agenticos2openclaw.sh --base-dir /opt/anolisa --mode recommended
```

Automatically install missing RPM dependencies when appropriate:

```bash
bash extension/openclaw/agenticos2openclaw.sh --mode recommended --install-deps
```

## Install Modes

| Mode | Modules | Use Case |
|---|---|---|
| `recommended` | osbase, skill, skillfs, ws-ckpt, skvm, sec-core, token-less | default installation for most users |
| `all` | osbase, skill, skillfs, ws-ckpt, sec-core, sight, token-less, skvm | full solution including Sight diagnostics |
| module list | any selected modules, such as sec-core, token-less, skillfs | targeted deployment and troubleshooting |

`openclaw` is the base dependency for OpenClaw-integrated modules. It is not shown as part of `recommended` or `all`, but the installer automatically adds it to the install queue when selected modules require OpenClaw and the `openclaw` command is not available.

`recommended` intentionally excludes Sight to keep the default installation lightweight. Use `all` when diagnostics and observability are required by default.

## Operations

List modules:

```bash
bash extension/openclaw/agenticos2openclaw.sh --list
```

Check status:

```bash
bash extension/openclaw/agenticos2openclaw.sh --status
```

Retry failed modules:

```bash
bash extension/openclaw/agenticos2openclaw.sh --retry
```

Roll back failed modules:

```bash
bash extension/openclaw/agenticos2openclaw.sh --rollback
```

Full rollback:

```bash
bash extension/openclaw/agenticos2openclaw.sh --rollback --full
```

## Best Practices

- Use `recommended` as the default production baseline.
- Use `all` only when full observability is required.
- Keep the Skill file concise and operation-focused; keep architecture and user documentation in `docs`.
- Prefer explicit `bash -s -- --mode recommended` for remote install commands.
- Run `--dry-run` before production rollout.
- Keep the installer idempotent: every module should have detect, install, verify, and cleanup behavior.
- Preserve OpenClaw config backups before plugin or config changes.
- Use `--base-dir` for non-standard Agentic OS component layouts.

## Troubleshooting

If installation fails:

1. Run `bash extension/openclaw/agenticos2openclaw.sh --status`.
2. Retry failed modules with `bash extension/openclaw/agenticos2openclaw.sh --retry`.
3. Check RPM availability with `rpm -q <package>`.
4. Re-run with `--install-deps` only when automatic RPM installation is acceptable.
5. Roll back failed modules with `bash extension/openclaw/agenticos2openclaw.sh --rollback`.
6. Use `--rollback --full` only when restoring the whole OpenClaw integration state is desired.

Common issues:

| Symptom | Likely Cause | Action |
|---|---|---|
| `jq` missing | prerequisite not installed | install `jq` and rerun |
| OpenClaw command missing | OpenClaw not installed or PATH not updated | install OpenClaw or rerun with OpenClaw module enabled |
| Skill source missing | Agentic OS RPMs not installed or custom root path | install component RPMs or pass `--base-dir` |
| token-less plugin not loaded | OpenClaw config path mismatch | pass `--config` and restart gateway |
| sec-core sandbox degraded | `linux-sandbox` missing | verify `agent-sec-core` RPM installation |

## Repository Layout

```text
extension/openclaw/
├── README.md
├── SKILL.md
└── agenticos2openclaw.sh
```
