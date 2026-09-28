import assert from 'node:assert/strict';
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const root = process.env.OPENCLAW_PACKAGE_DIR;
assert.equal(JSON.parse(readFileSync(resolve(root, 'package.json'))).version, '2026.9.6');
const file = readdirSync(resolve(root, 'dist')).find(name => /^hooks-.*\.mjs$/.test(name) && readFileSync(resolve(root, 'dist', name), 'utf8').includes('function createHookRunner('));
assert.ok(file);
const source = readFileSync(resolve(root, 'dist', file), 'utf8');
const alias = source.match(/createHookRunner as (\w+)/)?.[1];
assert.ok(alias);
const createHookRunner = (await import(pathToFileURL(resolve(root, 'dist', file))))[alias];
const { default: plugin } = await import('../../../adapters/openclaw/index.mjs');

function runner(socket, onError) {
  const typedHooks = [];
  plugin.register({
    pluginConfig: { binary: process.env.AW_BIN, socket, agent: 'openclaw', timeoutMs: 7000,
      hooks: { before: [{ provider: 'policy', onError }], after: [{ provider: 'policy', onError: 'report' }] } },
    on(hookName, handler, options) { typedHooks.push({ pluginId: plugin.id, hookName, handler, ...options }); },
  });
  // Match the pinned Gateway's global runner, including its before failure policy.
  return createHookRunner({ typedHooks, plugins: [], hooks: [] }, {
    catchErrors: true, failurePolicyByHook: { before_tool_call: 'fail-closed' },
  });
}

const event = command => ({ toolName: 'exec', params: { command }, toolCallId: 'openclaw-call' });
const context = { toolName: 'exec', sessionId: 'bridge-test' };
const live = runner(process.env.AW_SOCKET, 'block');
const allowed = await live.runBeforeToolCall(event('printf allowed'), context);
assert.notEqual(allowed?.block, true);
assert.equal(allowed?.requireApproval, undefined);
assert.equal(allowed?.params, undefined);
const denied = await live.runBeforeToolCall(event('printf AW_DENY_FIXTURE'), context);
assert.equal(denied.block, true);
assert.equal(denied.blockReason, 'AW policy blocked this tool call');
assert.equal(await live.runAfterToolCall({ ...event('printf allowed'), result: { content: [{ type: 'text', text: 'allowed' }] } }, context), undefined);
for (const policy of ['block', 'report']) {
  const result = await runner(process.env.AW_SOCKET + '.missing', policy).runBeforeToolCall(event('printf allowed'), context);
  assert.equal(result?.block === true, policy === 'block');
  assert.equal(result?.requireApproval, undefined);
}
writeFileSync(process.env.AW_BRIDGE_RESULT, JSON.stringify({
  framework: 'openclaw', version: '2026.9.6', scope: 'native hook runner and current AW plugin; no model',
  allow_neutral: true, native_block_adopted: true, after_observed: true,
  disconnected_block: true, disconnected_report: true,
}) + '\n');
