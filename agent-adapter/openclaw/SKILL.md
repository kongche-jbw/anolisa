---
name: agenticos-deploy
version: 0.2.0
description: Deploy and manage Agentic OS for OpenClaw. Use when the user asks to install, configure, upgrade, verify, retry, roll back, or troubleshoot Agentic OS capabilities for OpenClaw, including osbase, SKILL, skillfs, ws-ckpt, sec-core, Sight, token-less, and skvm.
layer: application
lifecycle: usage
---

# Agentic OS for OpenClaw Deploy

Use this skill to help users install and manage the integrated Agentic OS for OpenClaw solution.

## Installer

Download the installer from GitHub and run it with explicit arguments:

```bash
curl -fsSL "${AGENTICOS_INSTALLER_URL:-https://raw.githubusercontent.com/alibaba/anolisa/main/extension/openclaw/agenticos2openclaw.sh}" | bash -s -- --mode recommended
```

For local repository development, run:

```bash
bash extension/openclaw/agenticos2openclaw.sh --mode recommended
```

The installer requires Linux, Bash 4+, `jq`, and `npm` when OpenClaw itself must be installed.

## When To Use

Use this skill when the user asks for:

- one-click Agentic OS installation for OpenClaw
- OpenClaw integration with Agentic OS components
- installing or migrating Agentic OS Skills into OpenClaw
- enabling sec-core, Sight, osbase, runtime, ws-ckpt, token-less, or skvm for OpenClaw
- checking install status, retrying failed modules, or rolling back an installation

## Install Modes

Recommend `recommended` for most users:

```bash
curl -fsSL "${AGENTICOS_INSTALLER_URL:-https://raw.githubusercontent.com/alibaba/anolisa/main/extension/openclaw/agenticos2openclaw.sh}" | bash -s -- --mode recommended
```

`recommended` installs: `osbase`, `skill`, `skillfs`, `ws-ckpt`, `skvm`, `sec-core`, `token-less`.

Use `all` when the user explicitly wants full observability and every supported capability:

```bash
curl -fsSL "${AGENTICOS_INSTALLER_URL:-https://raw.githubusercontent.com/alibaba/anolisa/main/extension/openclaw/agenticos2openclaw.sh}" | bash -s -- --mode all
```

`all` installs: `osbase`, `skill`, `skillfs`, `ws-ckpt`, `sec-core`, `sight`, `token-less`, `skvm`.

Use module names for a targeted install:

```bash
curl -fsSL "${AGENTICOS_INSTALLER_URL:-https://raw.githubusercontent.com/alibaba/anolisa/main/extension/openclaw/agenticos2openclaw.sh}" | bash -s -- --mode sec-core token-less skillfs
```

Available modules:

| Module | Purpose |
|---|---|
| `openclaw` | OpenClaw Agent Gateway |
| `osbase` | system memory tuning for small OpenClaw instances |
| `skill` | migrate Agentic OS Skills into OpenClaw |
| `skillfs` | compact Skill filesystem for token reduction |
| `ws-ckpt` | workspace checkpoint and restore |
| `sec-core` | sandbox, asset verification, and security Skills |
| `sight` | diagnostics and observability for agent behavior |
| `token-less` | context compression and output filtering |
| `skvm` | skvm skill bank |

`openclaw` is a base dependency. The installer automatically adds it to the install queue when a selected module depends on OpenClaw and the `openclaw` command is not found.

## Operations

Preview the plan:

```bash
bash extension/openclaw/agenticos2openclaw.sh --mode recommended --dry-run
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

If RPM dependencies are missing and the user approves automatic package installation, add:

```bash
--install-deps
```

## Decision Guide

Choose `recommended` for first-time users and production defaults. It includes security, runtime acceleration, Skill migration, checkpoint support, token reduction, and small-instance tuning. It does not include Sight diagnostics.

Choose `all` when the user wants the full solution including Sight diagnostics.

Choose `sec-core` only for a minimal security-focused integration.

Choose `skill skillfs ws-ckpt skvm` when the user wants Skill/runtime improvements without security or token plugins.

Use `--base-dir DIR` when Agentic OS components are installed somewhere other than `/usr/share/anolisa`.

Use `--config FILE` when OpenClaw uses a non-default config path.

## Troubleshooting

If installation fails:

1. Run `bash extension/openclaw/agenticos2openclaw.sh --status`.
2. Retry with `bash extension/openclaw/agenticos2openclaw.sh --retry`.
3. Check missing RPMs with `rpm -q <package>`.
4. If RPMs are unavailable, rerun with `--install-deps` only after confirming with the user.
5. Roll back with `bash extension/openclaw/agenticos2openclaw.sh --rollback`.

Modules `sec-core` and `token-less` may require an OpenClaw gateway restart. The installer attempts this automatically in non-interactive mode and reports the manual command if restart fails.
