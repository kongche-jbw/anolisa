#!/usr/bin/env node
import { appendFileSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
const argument = name => process.argv[process.argv.indexOf(name) + 1];
const root = argument('--socket');
const provider = argument('--provider');
const input = JSON.parse(readFileSync(0, 'utf8'));
appendFileSync(join(root, 'hooks.jsonl'), JSON.stringify({ provider, ...input, pid: process.pid, time: Date.now() }) + '\n');
if (provider === 'large-output') {
  process.stdout.write(JSON.stringify({ payload: 'x'.repeat(2 * 1024 * 1024) }));
} else if (provider === 'environment') {
  const observed = { sentinel: process.env.AW_NATIVE_TEST_SENTINEL, cwd: process.cwd() };
  const content = [{ type: 'text', text: JSON.stringify(observed) }];
  process.stdout.write(JSON.stringify(input.hook === 'before_tool_call'
    ? { params: { ...input.event.params, ...observed } }
    : input.hook === 'tool_result_persist'
      ? { message: { ...input.event.message, content } }
      : { result: { ...input.event.result, content } }));
} else if (provider === 'rewrite' && input.event.toolName === 'exec') {
  process.stdout.write(JSON.stringify({ params: { ...input.event.params, command: 'node calculation.mjs --aw' } }));
} else if (provider === 'result' && input.event.toolName === 'exec') {
  process.stdout.write(JSON.stringify({ result: { ...input.event.result, content: [{ type: 'text', text: 'native-projected-value-73' }] } }));
} else if (provider === 'projection' && input.event.toolName === 'exec') {
  process.stdout.write(JSON.stringify({ message: { ...input.event.message, content: [{ type: 'text', text: 'native-projected-value-73' }] } }));
} else if (provider === 'ask') {
  process.stdout.write(JSON.stringify({ requireApproval: { title: 'AW native approval', description: 'Explicit ask for the harmless calculation', timeoutMs: 1000 } }));
} else if (provider === 'deny') {
  process.stdout.write(JSON.stringify({ block: true, blockReason: 'AW_NATIVE_DENY' }));
} else if (provider === 'failure') {
  process.stderr.write('fixture provider failed');
  process.exitCode = 9;
} else {
  process.stdout.write('{}');
}
