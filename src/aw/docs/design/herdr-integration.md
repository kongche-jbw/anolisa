# Pinned Herdr presentation integration

[中文版](herdr-integration_zh.md)

This integration presents verified AW results in unmodified Herdr v0.9.0. It
uses upstream pane metadata and configurable sidebar rows. Herdr does not invoke
Providers, verify receipts, install Agent plugins, or make security decisions.

## Version and distribution

[`upstream.json`](../../integrations/herdr/upstream.json) pins the official
non-prerelease tag `v0.9.0`, source commit
`b99002ac99b09e00b4ca692436cb15a6b0d676f1`, Linux ARM64 and x86_64 release
artifact digests, and the Apache-2.0 license digest. The older PoC snapshot is
not this release. No upstream patches or source copies are included.

The fetch helper verifies existing files before reuse, refuses mismatching
files, and saves the upstream license next to the executable. It downloads
only the selected architecture. This is release-artifact retrieval, not a
source-build reproducibility claim. Upgrades require explicitly changing the
pin and repeating compatibility tests. Automatic updates are disabled.

Run from `src/aw`; Python 3 and network access are required for the fetch:

```bash
timeout 120 python3 integrations/herdr/fetch.py target/herdr-integration/bin
PYTHONDONTWRITEBYTECODE=1 timeout 60 python3 integrations/herdr/smoke.py \
  target/herdr-integration/bin/herdr --output target/herdr-integration/smoke
PYTHONDONTWRITEBYTECODE=1 timeout 30 python3 -m unittest discover \
  -s integrations/herdr/tests -v
```

## Boundary between evidence and presentation

The launcher supplies `aw-view-cli` with a trusted binding file containing
runtime, complete session scope, Agent PID and Linux process start ticks, and
absolute journal/evidence paths. The Rust verifier validates source-bound
receipts and journal records. The bridge consumes its verified summary; it
never reads legacy Tokenless statistics or guesses adoption from a candidate.

Before and after verification, the bridge checks native `pane.get` session
identity and `pane.process_info` membership. The Agent PID must be the pane's
shell or a member of its foreground process group. A wrapper may be present;
an unrelated or detached process is rejected. A native session must already be
observed by Herdr or reported by the trusted launcher. Display labels are not
session bindings. A `/new` session requires a new trusted binding.

The private view has `format: 1`, exact `scope`, `runtime_alive: true`,
`verification: journal_verified`, and Provider counters with `kind: security`
or `projection`. Missing evidence yields zero calls only if the verifier
successfully checks the current binding. Invalid evidence, a stopped runtime,
or mismatched session clears the old counters and displays `AW unknown`.

`not_observed` adoption must have zero adopted calls and zero saved bytes.
When independently verified `local_history` evidence is present, the row says
`history adopted`; it does not claim the candidate reached a model request.
Calls, candidates, failed and bypassed results remain separate counters in
the view. The sidebar retains history adoption and saved bytes alongside
failed or bypassed calls from the same session. It also repeats failure and
bypass totals in the final row so that clipping a long Provider row does not
hide them. With no such outcomes, that row shows the detach hint. Candidates
remain visible when adoption is unknown. The sidebar never shows source
content, hashes, or private paths.

The bridge supports one security Provider and one projection Provider in the
compact layout. Unsupported kinds or ambiguous duplicate kinds fail visibly.
This is a presentation constraint, not a restriction on the AW Schema.

## Publishing to an existing managed pane

Use the dedicated Herdr socket and the exact pane ID selected by the launcher:

```bash
python3 integrations/herdr/bridge.py \
  --socket /path/to/isolated/api.sock --pane pane-id \
  --verifier "$PWD/target/debug/aw-view-cli" \
  --binding /path/to/launcher-owned-binding.json --duration 180
```

These are placeholders, not existing deployment paths. The watcher is bounded
to 1–3600 seconds; omit `--duration` to publish once. Reports use one source
`anolisa.aw`, increasing sequence numbers and a five-second TTL. Failure
clears all previous counters. A stopped watcher explicitly clears its tokens;
if it crashes, TTL expiry removes them. A single reporter owns each pane.

[`config.toml`](../../integrations/herdr/config.toml) provides five compact
rows: native Agent state, AW verification, SecCore, Tokenless, and adoption
scope/detach instructions. SecCore calls are observations, not OS protection.
AgentSight and Checkpoint are not advertised as activated. Provider settings
remain owned by the AW launcher; the panel does not hot-install native plugins.
Use an Agent normally after launching it with the existing opt-in AW hook
settings. `Ctrl+B`, then `q`, detaches; it does not stop the Agent.

## Isolation and acceptance scope

The smoke helper starts the pinned real binary with an independent API socket,
config/state directories and a no-auth Bash pane. A short temporary directory
keeps Unix socket paths within their platform limit. It records exact command,
PID, socket and stop command before validation, then terminates and waits for
only its owned process groups and removes that namespace. It never attaches
to an existing Herdr or reads Agent authentication. Its sidebar text explicitly
identifies a UI fixture; it is not AW execution or adoption evidence.

Validated here on Linux ARM64: release digest, native metadata read-back,
actual 120×40 and 80×24 TUI sidebar rendering, terminal input, keyboard detach with the
pane still alive, and metadata expiry. The helper retains ANSI output and
ownership/results JSON under its `--output` directory. x86_64 is pinned but
not runtime-tested here. Popup behavior, COSH lifecycle integration, clean VM
deployment and a redesigned settings panel are not part of this acceptance.

`live_smoke.py` is a separate composition runner. It accepts a trusted JSON
argv file (`--command-json`), creates an isolated real Herdr pane and appends
`--interactive`, `--herdr-socket`, and `--herdr-pane-id` for the Tokenless
acceptance command. The command must save `herdr-view.json` and
`herdr-metadata.json` in its new `--output-dir`. The runner requires verified
history adoption, matching metadata and the corresponding saved-byte text in
the real TUI. It cannot turn the no-auth fixture smoke into AW evidence.
Its real-Agent result is recorded separately from the UI compatibility result.

## Upstream references

- [Official immutable v0.9.0 release](https://github.com/herdrdev/herdr/releases/tag/v0.9.0)
- [Pinned pane metadata and process contracts](https://github.com/herdrdev/herdr/blob/b99002ac99b09e00b4ca692436cb15a6b0d676f1/src/api/schema/panes.rs)
- [Pinned sidebar configuration](https://github.com/herdrdev/herdr/blob/b99002ac99b09e00b4ca692436cb15a6b0d676f1/docs/preview/website/src/content/docs/configuration.mdx)
- [Pinned license](https://github.com/herdrdev/herdr/blob/b99002ac99b09e00b4ca692436cb15a6b0d676f1/LICENSE)

The pinned native session API accepts registered sources only. The Qoder launcher uses `source=herdr:qodercli`, `agent=qodercli`, a monotonic sequence and `session_start_source=startup`. An arbitrary source can receive an RPC acknowledgement without establishing a session binding. This namespace provides upstream interoperability; the owned launcher, process incarnation and native event still establish identity, not the source label.
