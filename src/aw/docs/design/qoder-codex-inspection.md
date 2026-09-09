# Qoder and Codex inspection integration

[中文版](qoder-codex-inspection_zh.md)

This increment connects native post-tool hooks to the shared adapter, Core and
an actual SecCore scanner. It provides an opt-in observation runner, not a
replacement for the existing security plugins or their policies.

## Execution path

`Qoder/Codex PostToolUse → aw-hook-cli → aw-adapters → aw-core → aw-sec-host → SecCore native protocol 1 → Receipt → FileJournal`

`aw-hook-cli` reads one native event from stdin. An explicit launcher-owned
configuration supplies the runtime binding, scope, Agent PID/start ticks,
Provider launch configuration and evidence directories. On Linux the runner
checks that it descends from the configured live Agent incarnation. Native
session, tool and Codex turn IDs are bound to that context; mismatches fail.
This is local process correlation, not OS enforcement or protection from another
process with the same user's privileges.

Qoder's documented hook payload does not provide a turn ID. Its smoke profile
therefore requires an explicitly supplied launcher-owned **single turn**. It
cannot be used as a general multi-turn identity source. Codex uses its native
`turn_id`. Stable occurrence keys prevent a repeated tool event from being
executed again through the same journal.

The runner constructs a pinned `security.content.inspect/v2` plan. A required
Provider failure rejects that plan and retains the original native result. A
sensitive result emits a native `systemMessage` observation; it does not request
approval, block dispatch, redact text or claim adoption. Existing security
plugins must retain their own policy behavior until separately migrated.
Do not register both old and new inspection paths for the same capability
without explicitly addressing duplicate scans and notifications.

## Real Provider, unchanged contracts

`aw-sec-host` invokes the existing SecCore native protocol 1 with an explicit
program, argument vector and environment. It reuses SecCore's scanner and
native response vocabulary. No detector or security policy is implemented in
AW. The checked Provider source is the PoC branch at `5ebfc0b3`, package
`agent-sec-cli` 0.11.0, under
`src/agent-sec-core/agent-sec-cli/src/agent_sec_cli/aw_provider/`.
That source and its Python 3.11.6 environment are external dependencies of this
integration; this commit does not silently import the old PoC AW Core or v1
canonical contracts. The installed system CLI is not assumed to supply this
native Provider entrypoint.

The Host maps scanner-reported byte coverage, truncation and findings into the
existing v2 output. Missing or inconsistent coverage fails; partial coverage
never becomes a clean complete scan. Error, skipped and produced responses
remain distinct. Receipt timestamps and input/output digests are generated
from the actual invocation and process exchange.

The descriptor pins the explicit configuration and executable bytes. It does
not attest all Python imports or installed dependencies. For Python, use `-P`
to keep the Agent workspace from shadowing the configured Provider module.
The child gets only the explicit environment. Linux process groups, bounded
nonblocking pipes and deadlines limit the exchange; the Host does not install
an OS sandbox and does not contain a malicious Provider that escapes its group.

The runner verifies the journal acknowledgement before writing a per-event
summary containing scope, input digest/size, receipts and inspection output.
Raw tool content is not written to the execution journal or summary. Scanner
stderr is not copied into user-facing diagnostics. Failure before settlement
may leave a reservation without a final summary; never blindly retry it.
All previously registered Schema resources and the Core implementation stay
unchanged.

## Build and exercise

From `src/aw`:

```bash
cargo build -p aw-hook-cli --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo doc --workspace --no-deps --locked
python3 tests/check_canonical.py
python3 tests/native_smoke.py --help
```

The smoke script requires explicit absolute paths to `aw-hook-cli`, the
Provider's Python executable and its source directory, plus a new output
directory. It defaults to isolated Agent configuration and optionally reuses
the current Qoder login in place. Neither mode copies authentication,
and preserves run evidence. Review its command and lifecycle record before
interpreting a run. The Codex model is a local **scripted Responses fixture**;
the Codex binary, shell tool, AW Core, scanner and journal are real. This tests
native integration without asserting real remote-model reasoning.

Qoder defaults to isolated configuration and no session persistence. With
`--qoder-existing-login` it uses the current login in place and loads a dedicated
project hook in the test workspace.
Authentication absence is reported as blocked; a plugin stdin fixture is not a
substitute for a Qoder runtime acceptance. Neither test proves Tokenless
compression, final adoption, Herdr metrics, pre-tool enforcement or clean-VM
installation.

## Native source references

- [Codex hooks](https://developers.openai.com/codex/hooks): matched command hooks
  may run concurrently; registration order is not an AW serial execution plan.
- [Codex 0.153.4 native tool hook tests](https://github.com/openai/codex/blob/rust-v0.153.4/codex-rs/core/tests/suite/hooks.rs#L5066):
  shell execution maps to `Bash` and string post-tool output with `turn_id`.
- [Qoder hooks](https://docs.qoder.com/cli/hooks): tool-dependent result shapes
  and `hookSpecificOutput.updatedToolOutput` for post-tool replacement.

Qoder replacement remains a separate future delivery integration. Codex does
not expose that same general replacement contract. Neither framework is assigned
final-guard or adoption capabilities merely because a hook ran.

## Checked runtime and reproduction command

The checked versions are Qoder CLI 1.1.5 and Codex CLI 0.153.4 on Linux ARM64.
Codex passed sensitive (44 bytes), clean (34 bytes) and injected subprocess
failure cases. The failure case deliberately exits before the scanner and
requires a failed Receipt, no output and Core `preserve`; it is not a scanner
success. Successful cases require a produced Receipt, matching coverage and
Core `proceed`. Qoder initially stopped at the empty configuration login
boundary. A later run reused the existing login and verified its real model
and Bash hook: 43 sensitive bytes, complete coverage and a produced Receipt.
That Qoder run did not use the scripted model.

The default read-only Codex sandbox failed locally with a bubblewrap loopback
permission error. The successful runs explicitly used `--sandbox
danger-full-access` for the fixed scripted `printf` only. No sandbox guarantee
was tested; there is no automatic fallback in the script.

After preparing the pinned Provider checkout and its Python dependencies, run
this from `src/aw`, substituting the two explicit Provider paths:

```bash
python3 tests/native_smoke.py \
  --host codex --case sensitive --sandbox danger-full-access \
  --provider-version 0.11.0 \
  --hook-bin "$PWD/target/debug/aw-hook-cli" \
  --provider-python /absolute/provider-checkout/src/agent-sec-core/agent-sec-cli/.venv/bin/python \
  --provider-source /absolute/provider-checkout/src/agent-sec-core/agent-sec-cli/src \
  --output-dir "$PWD/target/codex-sensitive-run"
```

Use `clean` or `provider-failure` with a fresh output directory for the other
cases. Use `--host qoder` without the sandbox option for the isolated login
probe. Each run records its command, PIDs, port, logs and stop commands in
`lifecycle.json`; the final `result.json` distinguishes passed, failed and
blocked. Model requests contain only the synthetic test conversation. Owned
Agent homes are removed after each run; logs, settings, receipts and journal
remain in the explicit output directory for review. Remove that directory to
clean its evidence, or use `cargo clean` to remove all AW build/test outputs.
This does not validate deployment on a clean machine.

## Reuse the current Qoder login

In the reproduction command use `--host qoder --qoder-existing-login`, omit the
Codex `--sandbox` option and choose a fresh output directory. This option does
not copy, recreate or delete the user's login configuration. It creates only a
project hook, disables its own session persistence and cleans only its temporary
directories. Existing interactive sessions are not restarted.

The observed Qoder 1.1.5 Bash result is an object containing stdout, stderr,
exit code and status fields. The adapter accepts only completed, successful,
non-image results with empty stderr and extracts exact stdout bytes. Qoder had
already removed the command's trailing newline, so AW inspects the received
43 bytes without reconstructing 44 bytes. Other metadata is preserved; this
does not replace text or establish adoption.

The test stores one synthetic call's `native-event.json` and checks its stdout
against the AW input digest. It reads no historical session. Qoder still uses
a dedicated single-turn identity; general multi-turn integration, existing
plugin coexistence and pre-tool enforcement remain unverified.
