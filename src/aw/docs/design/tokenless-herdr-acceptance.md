# Tokenless and Herdr acceptance

[中文版](tokenless-herdr-acceptance_zh.md)

This integration extends the local Qoder/Codex inspection baseline. Qoder runs
SecCore and Tokenless in one AW plan; an independent observer verifies the
persisted native tool-result slot. Herdr displays only session-bound, verified
results. No schema resources or Core implementation were changed.

## Build and run

Use Linux ARM64 for the validated environment. Source Tokenless is 0.8.0 with
native protocol v2; the system's older 0.7.0 binary is unsuitable. Herdr is pinned
to the unmodified official v0.9.0 release. Qoder acceptance uses CLI 1.1.47 and
an existing login in place. SecCore still requires the explicitly selected
0.11.0 native Provider source and Python 3.11.6 environment described in
[the inspection integration](qoder-codex-inspection.md).

From the repository root:

```bash
cargo build --manifest-path src/tokenless/Cargo.toml -p tokenless-cli --locked
cd src/aw
cargo build -p aw-hook-cli --bins --locked
python3 integrations/herdr/fetch.py target/herdr-integration/bin
```

From `src/aw`, replace the two absolute Provider paths and choose a new output
directory on every run:

```bash
PYTHONDONTWRITEBYTECODE=1 timeout 150 python3 tests/tokenless_smoke.py \
  --case compressed \
  --hook-bin "$PWD/target/debug/aw-hook-cli" \
  --view-bin "$PWD/target/debug/aw-view-cli" \
  --adoption-bin "$PWD/target/debug/aw-adoption-cli" \
  --tokenless-bin "$PWD/../tokenless/target/debug/tokenless" \
  --provider-python /absolute/provider-checkout/src/agent-sec-core/agent-sec-cli/.venv/bin/python \
  --provider-source /absolute/provider-checkout/src/agent-sec-core/agent-sec-cli/src \
  --output-dir "$PWD/target/tokenless-run"
```

Cases are `compressed`, `no-gain` and `provider-failure`. All run exactly one
synthetic read-only `cat fixture.json`; the last case deliberately exits the
Tokenless process before compression. The runner explicitly consents to
unrecoverable replacement. It rejects installed Qoder plugins or user hooks instead of silently disabling
existing integrations. Print-mode tests use project-only settings; interactive
tests use default setting sources and explicitly select the existing login's
`auto` model. Neither uses the system Tokenless binary or global statistics.

For the real Herdr combination, save the command above as a JSON argv array,
using absolute paths and a fresh `--output-dir`, then run:

```bash
PYTHONDONTWRITEBYTECODE=1 timeout 210 python3 integrations/herdr/live_smoke.py \
  target/herdr-integration/bin/herdr \
  --output target/herdr-integration/live-run \
  --command-json /absolute/owned-command.json
```

The helper starts an isolated Herdr server and TUI, enters Qoder interactively,
trusts only the fresh test workspace, and submits the single prompt using native
terminal key events. With `--setting-sources project`, the tested interactive `/hooks` view listed
zero project hooks. Interactive acceptance therefore uses default sources after
checking that user hooks/plugins are empty, initializes a dedicated Git root,
and submits the prompt after startup instead of using `-i`. Agent
PID/start ticks must match the actual pane foreground process, and the launcher
reports the captured session ID before publishing verified counters.

## Evidence and review points

- `native-event.json` binds the successful Bash stdout, session and tool ID.
- `evidence/<event>.json` contains the plan, invocation metadata, Receipt,
  candidate and Core journal tip. Input content is reconstructed from the
  captured native event when verifying adoption; hook delivery remains a
  separate `prepared_for_return` observation.
- `adoption-journal/` and `<event>.adoption.json` retain only the two matching
  native history rows and the existing AW adoption contract. `verify` checks
  them against the full plan and execution after the original test session has
  been removed. This trusts the local recorder/storage owner, not a signature
  against a malicious process with the same user permissions.
- `view-binding.json` selects a native session. Its optional
  `adoption_bindings` maps each event key to that event's trusted adoption
  binding file, so one call's native capture cannot be reused for another.
  Only independently verified adoptions count toward savings. Journal damage
  or a wrong binding fails visibly; unrelated sessions are excluded.
- `herdr-view.json`, `herdr-metadata.json` and the real `screen.ansi` correlate
  the verifier output, native metadata and rendered sidebar. The label is
  `history adopted`, never model consumption. Mixed failures or bypasses do
  not erase previously verified savings.

The capture owns one explicitly bounded Qoder turn. General multi-turn launch
identity, native plugin coexistence, automatic COSH lifecycle integration,
checkpoint actions, OS isolation and clean-machine packaging remain separate
work. Codex retains its inspection path and rejects Tokenless projection;
its hook does not provide this Qoder replacement contract. ARM64 is validated;
x86_64 Herdr artifacts are pinned but have not been executed here.

Each runner records commands, versions, PID/start ticks, paths and stop commands
in `lifecycle.json` or `ownership.json`. Successful cleanup removes only the
fresh UUID session, its state directory, temporary home and the test workspace
trust entry. Authentication is neither copied nor removed. Logs and synthetic
proofs stay in the chosen output directory. No shared VM or existing Herdr is
restarted. To discard all generated material in this worktree, run `cargo clean`
with each of the AW and Tokenless Cargo manifests; save useful evidence first.

## Recorded results

| Case | Observed result |
| --- | --- |
| Qoder + Herdr combination | 4459 B → 3107 B; one local-history adoption, 1352 B saved; native metadata and actual TUI agree |
| Qoder no gain | Original retained; zero adoptions and zero saved bytes |
| Qoder Tokenless fault | Failed Receipt, Core preserve; native history retains original, zero savings |
| Codex inspection regression | Real CLI → SecCore succeeds; the second scripted model request retains the original tool output |

All 144 Rust tests and 15 Herdr Python tests passed, along with fmt, Clippy,
documentation build and cross-language digest checks. Combined evidence is under
`target/tokenless-herdr/qoder-combined/` and `target/herdr-integration/combined/`.
No-gain, fault and Codex evidence use sibling `qoder-no-gain-verified/`,
`qoder-provider-failure/` and `codex-regression/` directories. Earlier diagnostic
runs are retained for configuration/binding investigation, not acceptance proof.
All test processes have ended.
