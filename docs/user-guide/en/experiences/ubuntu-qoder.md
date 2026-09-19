# Two short Qoder experiences on Ubuntu

[中文版](../../zh/experiences/ubuntu-qoder.md)

Show an unchanged RPM-style Skill becoming usable on Ubuntu, then show Qoder
diagnosing a CI report with less tool output. Each activity takes 2–3 minutes
after the organizer finishes setup. Downloading components and account login
are preparation, not part of the participant's timer.

| Theme | Display tags | Introduction | Materials | Slogan | Minutes |
| --- | --- | --- | --- | --- | --- |
| SkillFS cross-distribution Skills | Read-time adaptation; unchanged source | Read the same RPM-style Skill in two Ubuntu workspaces, compare commands and confirm the source hash. | Side-by-side terminal screenshots; source hash; group QR code | Change the system. Keep the Skill. | 2–3 |
| Tokenless tool-output savings | Log compression; failure diagnosis | Ask Qoder to diagnose the same CI report with compression off and on, then inspect measured tool-content savings. | Full/shortened report; statistics; group QR code | Keep the failure. Trim the test log. | 2–3 |

## Prepare a clean machine

Use Ubuntu 22.04 or 24.04 **x86_64**, a normal user with sudo, network access,
and an accessible `/dev/fuse`. The published SkillFS raw package currently
supports x86_64 and system installation. ARM machines need a separate source
build; the bootstrap deliberately stops on ARM rather than installing a wrong
binary. No RPM package manager is needed on Ubuntu.

Clone the organizer's fork branch and run its bootstrap:

```bash
git clone --depth 1 --branch feature/skillfs/ubuntu-experience --single-branch \
  https://github.com/kongche-jbw/anolisa.git anolisa-experience
cd anolisa-experience
bash scripts/ubuntu-experience/bootstrap.sh
export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"
```

Install Git first with `sudo apt-get update && sudo apt-get install -y git`
if it is missing. Bootstrap uses the official anolisa installer and these
component commands:

```bash
sudo "$HOME/.local/bin/anolisa" --install-mode system install skillfs \
  --backend raw --version 0.4.2
anolisa --install-mode user install tokenless --backend raw --version 0.8.2
```

It installs anolisa 0.3.12, FUSE3, Python/pytest, and Qoder if missing. It then
creates the demo state and runs local checks. Each network/install command has
a timeout; a failure stops preparation. The installer does not silently build
from source or install unrelated components. Re-running bootstrap may
reconcile the pinned component installs; it retains an existing Qoder binary.

Qoder must support `--config-dir`, `--plugin-dir`, and
`--no-session-persistence` (the runner checks these). Record the actual Qoder
version during the rehearsal. Keep that version for the event.

## Configure Token Plan once

```bash
python3 scripts/ubuntu-experience/demo.py auth
```

Complete Qoder sign-in if requested. In `/model`, open **Custom**, select
**Add custom model**, and choose the Alibaba Cloud provider, the purchased
Token Plan and its model according to the live catalog. Enter the key in the
wizard and select the resulting model; exit Qoder when finished. The setup
window is limited to 15 minutes and can be reopened. Do not substitute a
pay-as-you-go endpoint or a different subscription product just to get past a
catalog/credential error. Resolve missing Token Plan support before the event.

Qoder's [custom model guide](https://docs.qoder.com/cli/custom-models) requires
the wizard and advises against writing BYOK fields into `settings.json`.
The [Token Plan overview](https://docs.modelstudio.console.alibabacloud.com/en/model-studio/token-plan-overview)
lists Qoder among supported tools. Available options still depend on the
account's catalog. No API key belongs in Git, shell commands, or event screenshots.

The event has its own Qoder config root. Credentials are retained across
participant resets. Optionally pin the exact custom model ID for every run:

```bash
export EXPERIENCE_MODEL='your-custom-model-id'
```

Tokenless is configured only for the event process: `--plugin-dir` loads the
native plugin shipped by `anolisa install tokenless`. Both A/B runs load the
same plugin; `TOKENLESS_COMPRESSION_ENABLED=0/1` is the only compression switch.
This avoids changing the user's global plugin registration. For ordinary
daily use outside this event, the supported persistent setup is
`anolisa adapter enable tokenless qoder` followed by restarting Qoder.

## Activity 1: same Skill, Ubuntu commands

**Sign text:** “Change the system. Keep the Skill.”

| Time | Participant action | Visible result |
| --- | --- | --- |
| 0:00–0:20 | Inspect the original Skill shown by the runner | `rpm -ql bash` and `dnf install -y tree` |
| 0:20–1:10 | Run the raw workspace | Qoder reads the original instructions and attempts the RPM query |
| 1:10–2:00 | Run the mounted workspace with the same prompt | Qoder reads `dpkg -L bash` and an `apt-get` installation plan |
| 2:00–2:30 | Compare the answer and source hash | Source unchanged; the Ubuntu query returns the shell path |
| 2:30–3:00 | Scan the organizer's group QR code | Invite participants to bring their own cross-distribution Skills |

```bash
python3 scripts/ubuntu-experience/demo.py skillfs raw
python3 scripts/ubuntu-experience/demo.py skillfs adapted
```

The task queries installed files and prints an installation plan; it does not
install tree. On a clean Ubuntu host, RPM will normally be missing. A capable
model might correct the raw instructions itself: acknowledge that recovery
rather than promise a baseline failure or latency win. The deterministic
comparison is the **bytes read through SkillFS and the unchanged source hash**.

The adapter is explicitly enabled with `target_os = "auto"`; the directive
stage is disabled. Normal mounts expose `<mount>/skills`, which is linked into
the adapted workspace's `.qoder/skills`. Each adapted run starts a foreground
FUSE worker and unmounts it on completion, interruption or timeout. It does not
leave a supervisor running. The official 0.4.2 CLI has no `--read-only` flag;
the prompt uses only read-only operations and the runner checks the source hash.
Only `SKILL.md` is transformed, using the shipped eligible rules; arbitrary
scripts and every Red Hat command are not automatically portable.

## Activity 2: find the failure in a shorter CI report

**Sign text:** “Keep the failure. Trim the test log.”

Preparation runs a tiny real pytest suite with 80 passing cases and one
intentional free-shipping boundary failure. The same captured output is copied
into both workspaces. `python3 ci_report.py` retrieves that report successfully;
the report still says **1 failed, 80 passed**. It does not claim that a new test
execution passed. Tokenless preserves failed tool executions, so directly
running a failing pytest command would not be a reliable compression demo.

| Time | Participant action | Visible result |
| --- | --- | --- |
| 0:00–0:15 | Hear the task: find the free-shipping boundary bug | One concrete question |
| 0:15–1:00 | Run baseline | Qoder receives the full report |
| 1:00–1:45 | Run optimized | Qoder receives a shorter report with the failure preserved |
| 1:45–2:30 | View statistics and compare answers | Same failed test, fewer estimated tool-content tokens |
| 2:30–3:00 | Scan the group QR code | Invite a real build/test output for follow-up |

```bash
python3 scripts/ubuntu-experience/demo.py tokenless baseline
python3 scripts/ubuntu-experience/demo.py tokenless optimized
python3 scripts/ubuntu-experience/demo.py stats
```

Expected diagnosis: `test_free_shipping_at_threshold` fails at subtotal 100;
the fee is 5 instead of 0. Change the condition in `shipping_fee` from `<= 100`
to `< 100`. Do not edit during the timed activity. Both runs have identical
fixtures, prompts, model selection, output limits, and fresh sessions. Each
model run is limited to 90 seconds without model-request retries; preflight
both live pairs to check whether the network/model fits the event timing.

The optimized run must produce measured savings or the runner reports an
error. Statistics cover tool-content estimates, not the full model context,
wall-clock speed or the Token Plan bill. Baseline dry-run records are excluded
from normal summaries; read **Before → After** in the optimized records.
Do not promise a fixed percentage or use model self-reported token counts.

## Rehearse and reset

```bash
# No model call or key needed: real FUSE read + shipped Qoder hook contract.
python3 scripts/ubuntu-experience/demo.py check

# Tokenless-only local rehearsal; not a live Qoder/model session.
python3 scripts/ubuntu-experience/demo.py rehearsal

# After every participant, with no demo still running:
python3 scripts/ubuntu-experience/demo.py reset

# At the end of the event, also remove the event's Qoder credentials:
python3 scripts/ubuntu-experience/demo.py cleanup
```

Use `check`, then complete `auth` and both live pairs before opening the booth.
A passing local hook rehearsal does not prove the signed-in Qoder session
loaded the plugin. The optimized live run checks for recorded savings. Re-run
`reset` after rehearsing so participant statistics start empty.

The default state directory is
`$HOME/.local/share/anolisa-experience` (mode 0700). `reset` recreates `runtime/`
and its four workspaces, captured reports, answers and per-run databases. It
retains `qoder-config/`, `skills/`, `source.sha256` and `skillfs.toml`.
`cleanup` deletes this entire owned directory. Installed OS packages,
anolisa/Qoder binaries and components remain available for the next event.
There are no listening ports or persistent demo services.

An ownership marker, a process lock and mount checks prevent reset from
deleting an unrelated directory, an active run or a mounted filesystem.
Source drift causes reset to stop for inspection. Startup failure, interruption
and timeout stop the owned child process; logs remain for inspection. After
an uncatchable process kill, inspect `runtime/mount-process.json` or
`runtime/qoder-process.json` and verify the recorded PID/command before using
the recorded stop command. For a remaining mount at the default location:

```bash
fusermount3 -u "$HOME/.local/share/anolisa-experience/runtime/mount"
python3 scripts/ubuntu-experience/demo.py reset
```

Do not delete a still-mounted directory. To remove the installed components
after a dedicated event machine is retired, use their matching scopes:

```bash
anolisa --install-mode user uninstall tokenless
sudo "$HOME/.local/bin/anolisa" --install-mode system uninstall skillfs
```

For another demo location, put `--root /absolute/path` **before** the action on
every invocation. `--skillfs /path/to/skillfs` and `--adapter-dir /path/to/adapters`
support local release validation without replacing installed binaries.

## Organizer handoff

- Status: scripts ready for machine/account rehearsal; live Token Plan calls
  require the organizer's wizard login.
- Started: only command-scoped FUSE/Qoder children; no ports or services.
- Changed: event scripts/fixtures and this bilingual guide; no component code.
- Validation: Ubuntu ARM64 local validation with SkillFS built from tag
  `skillfs/v0.4.2`, the official Tokenless 0.8.2 ARM64 binary and Qoder 1.1.47
  plugin validation. FUSE conversion and source hash passed. Local hook replay
  reduced the sample from 7,349 to 1,369 characters (81.4%) while retaining the
  failure. This is a rehearsal measurement, not a promised live result.
  Eight ownership/reset/process-cleanup tests, shell syntax, Python formatting,
  bilingual parity and documentation links also passed. Qoder loaded the native
  plugin with its PostToolUse hook; interrupting a mounted run removed the real
  FUSE mount and reaped both the worker and the simulated Qoder child.
- Cleanup/remaining: the organizer's machine still needs the x86_64 bootstrap,
  account login and two live pairs. Use `reset` between participants and
  `cleanup` after the event; both commands are shown above.
