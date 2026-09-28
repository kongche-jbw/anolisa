#!/usr/bin/env python3
"""Exercise planner/runner methods extracted from the checked official binary.

This is a source contract test, not evidence of model-driven tool adoption.
Only timeout plumbing and the hook callback are replaced by the unit harness.
"""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path)
    args = parser.parse_args()
    root = args.root.resolve()
    binary = (root / "bin/qodercli").read_bytes()
    receipt = json.loads((root / "installation.json").read_text())
    assert hashlib.sha256(binary).hexdigest() == receipt["binary_sha256"]
    source = binary.decode("utf-8", errors="replace")
    planner = source.split("class hv{", 1)[1].split("}import*as JBe", 1)[0]
    methods = source.split("async executeHooksParallel(", 1)[1].split("}async executeRuntimeHook(", 1)[0]
    unit = """
import assert from 'node:assert/strict';
const e = {debug() {}};
const MT = JSON.stringify;
const Qx = x => [x];
const pi = x => x;
const aa = x => x;
const pdt = 0;
const nue = x => x.timeout ?? 1;
const cOe = String;
const uOe = f => f(undefined);
""" + "class NativePlanner {" + planner + "}\n" + \
        "class NativeRunner {async executeHooksParallel(" + methods + "}}\n" + """
function plan(entries) {
  return new NativePlanner({getHooksForEvent:()=>entries,
    getInlineSkillSessionHooksForEvent:()=>[]}).createExecutionPlan('PreToolUse',
      {toolName:'Bash', toolInput:{command:'original'}});
}
const entries = ['one','two'].map(name=>({enabled:true,matcher:'Bash',
  config:{type:'command',command:name}}));
assert.equal(plan(entries).sequential, false);
assert.equal(plan([{...entries[0],sequential:true},entries[1]]).sequential, true);
assert.equal(plan([{...entries[0],async:true},entries[1]]).asyncHookConfigs.length, 1);
assert.equal(plan([entries[0],entries[0]]).hookConfigs.length, 1);
const native = new NativeRunner();
native.shouldRunCompletedWriteAfterCancellation = () => false;
let trace=[];
native.executeHook=async (hook,event,input)=>{
  trace.push({name:hook.command,phase:'start',input:structuredClone(input)});
  await new Promise(resolve=>setTimeout(resolve,30));
  trace.push({name:hook.command,phase:'end'});
  return {success:true,output:{hookSpecificOutput:{hookEventName:event,
    updatedInput:{command:hook.command}}}};
};
const hooks=entries.map(e=>e.config);
const input={tool_input:{command:'original',description:'retained'}};
await native.executeHooksParallel(hooks,'PreToolUse',input);
assert.deepEqual(trace.map(x=>x.phase),['start','start','end','end']);
assert.equal(trace[1].input.tool_input.command,'original');
trace=[];
await native.executeHooksSequential(hooks,'PreToolUse',input);
assert.deepEqual(trace.map(x=>x.phase),['start','end','start','end']);
assert.equal(trace[2].input.tool_input.command,'one');
assert.equal(trace[2].input.tool_input.description,'retained');
const original={tool_response:{stdout:'original'}};
assert.deepEqual(native.applyHookOutputToInput(original,
  {hookSpecificOutput:{updatedToolOutput:'replacement'}},'PostToolUse'),original);
console.log(JSON.stringify({source_contract:true,default_parallel:true,
  any_matching_sequential_makes_all_sync_sequential:true,
  sequential_pretool_input_merged:true,sequential_posttool_response_not_chained:true,
  async_groups_separate:true,deduplicates_identical_hooks:true}));
"""
    test = root / "installed-contract.mjs"
    test.write_text(unit)
    result = subprocess.run(["node", str(test)], check=True, capture_output=True,
                            text=True, timeout=10)
    report = json.loads(result.stdout)
    report.update(version=receipt["version"], binary_sha256=receipt["binary_sha256"])
    (root / "source-contract-result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
