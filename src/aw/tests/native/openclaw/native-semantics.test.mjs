import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { readFileSync, readdirSync, mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { promisify } from 'node:util';
import test from 'node:test';

const testState = resolve('target/native-lab/openclaw/test-state');
mkdirSync(testState, { recursive: true });
const testConfig = resolve(testState, 'openclaw.json');
writeFileSync(testConfig, JSON.stringify({ logging: { file: resolve(testState, 'runtime.log'), level: 'warn' } }));
process.env.OPENCLAW_HOME = testState;
process.env.OPENCLAW_STATE_DIR = testState;
process.env.OPENCLAW_CONFIG_PATH = testConfig;

const packageRoot = resolve(process.env.OPENCLAW_PACKAGE_DIR ?? 'target/native-lab/openclaw/runtime/node_modules/openclaw');
assert.equal(JSON.parse(readFileSync(resolve(packageRoot, 'package.json'))).version, '2026.9.6');
const hooksFile = readdirSync(resolve(packageRoot, 'dist')).find(name => /^hooks-.*\.mjs$/.test(name) && readFileSync(resolve(packageRoot, 'dist', name), 'utf8').includes('function createHookRunner('));
assert.ok(hooksFile, 'Pinned official OpenClaw must expose its native hook runner');
const hookSource = readFileSync(resolve(packageRoot, 'dist', hooksFile), 'utf8');
const alias = hookSource.match(/createHookRunner as (\w+)/)?.[1];
assert.ok(alias, 'Find the actual native export; do not substitute a test runner');
const createHookRunner = (await import(pathToFileURL(resolve(packageRoot, 'dist', hooksFile))))[alias];
const event = () => ({ toolName: 'exec', params: { command: 'printf original' }, toolCallId: 'native-lab' });
const hook = (hookName, pluginId, priority, handler) => ({ hookName, pluginId, priority, handler });
const runner = (typedHooks, options = {}) => createHookRunner({ typedHooks }, { catchErrors: false, ...options });
const sleep = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));

test('before hooks await commands in descending priority and preserve original event snapshots', async () => {
  const seen = [];
  const run = runner([
    hook('before_tool_call', 'lower', 10, async e => { seen.push(['lower', e.params.command]); return { params: { command: 'printf lower' } }; }),
    hook('before_tool_call', 'higher', 20, async e => {
      seen.push(['higher-start', e.params.command]);
      const { stdout } = await promisify(execFile)('/bin/sh', ['-c', 'printf command-hook-completed'], { timeout: 2000 });
      seen.push(['higher-end', stdout]);
      e.params.command = 'mutation must remain local';
      return { params: { command: 'printf higher' } };
    }),
  ]);
  const input = event();
  const result = await run.runBeforeToolCall(input, {});
  assert.deepEqual(seen, [['higher-start', 'printf original'], ['higher-end', 'command-hook-completed'], ['lower', 'printf original']]);
  assert.equal(result.params.command, 'printf lower');
  assert.equal(input.params.command, 'printf original');
});

test('same-priority hooks preserve registration order and terminal block skips remaining hooks', async () => {
  const order = [];
  const run = runner(['first', 'deny', 'skipped'].map(id => hook('before_tool_call', id, 0, () => {
    order.push(id);
    return id === 'deny' ? { block: true, blockReason: 'lab deny' } : undefined;
  })));
  assert.equal((await run.runBeforeToolCall(event(), {})).blockReason, 'lab deny');
  assert.deepEqual(order, ['first', 'deny']);
});

test('ask remains a native approval request and freezes the first approved parameter snapshot', async () => {
  const run = runner([
    hook('before_tool_call', 'ask-owner', 30, () => ({ params: { command: 'printf reviewed' }, requireApproval: { title: 'AW ask', description: 'Explicit native approval', timeoutMs: 1000 } })),
    hook('before_tool_call', 'later', 20, () => ({ params: { command: 'printf unreviewed' }, requireApproval: { title: 'second', description: 'must not win' } })),
  ]);
  const result = await run.runBeforeToolCall(event(), {});
  assert.equal(result.requireApproval.pluginId, 'ask-owner');
  assert.equal(result.requireApproval.title, 'AW ask');
  assert.equal(result.params.command, 'printf reviewed');
  assert.notEqual(result.block, true);
});

test('a later deny overrides an earlier native ask', async () => {
  const run = runner([
    hook('before_tool_call', 'ask', 20, () => ({ requireApproval: { title: 'ask', description: 'review' } })),
    hook('before_tool_call', 'deny', 10, () => ({ block: true, blockReason: 'denied' })),
  ]);
  assert.equal((await run.runBeforeToolCall(event(), {})).block, true);
});

test('after hooks start concurrently, settle all handlers, and discard return values', async () => {
  const order = [];
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  const run = runner([
    hook('after_tool_call', 'slow', 20, async () => { order.push('slow-start'); await gate; order.push('slow-end'); return { result: 'ignored' }; }),
    hook('after_tool_call', 'fast', 10, async () => { order.push('fast-start'); release(); order.push('fast-end'); }),
  ]);
  assert.equal(await run.runAfterToolCall({ ...event(), result: 'original' }, {}), undefined);
  assert.deepEqual(order, ['slow-start', 'fast-start', 'fast-end', 'slow-end']);
});

test('result persistence is synchronous and chains transcript content', () => {
  const order = [];
  const run = runner([
    hook('tool_result_persist', 'first', 20, e => { order.push('first'); return { message: { ...e.message, content: [{ type: 'text', text: 'projected' }] } }; }),
    hook('tool_result_persist', 'second', 10, e => { order.push(e.message.content[0].text); return { message: { ...e.message, content: [{ type: 'text', text: e.message.content[0].text + '-twice' }] } }; }),
  ]);
  const result = run.runToolResultPersist({ message: { role: 'toolResult', toolCallId: 'lab', toolName: 'exec', content: [{ type: 'text', text: 'raw' }] } }, {});
  assert.equal(result.message.content[0].text, 'projected-twice');
  assert.deepEqual(order, ['first', 'projected']);
});

test('result persistence ignores asynchronous handlers with a visible warning', async () => {
  const warnings = [];
  const run = runner([hook('tool_result_persist', 'invalid-async', 0, async e => ({ message: { ...e.message, content: [] } }))], { catchErrors: true, logger: { warn: message => warnings.push(message), error: message => warnings.push(message) } });
  const message = { role: 'toolResult', content: [{ type: 'text', text: 'unchanged' }] };
  const result = run.runToolResultPersist({ message }, {});
  assert.equal(result.message.content[0].text, 'unchanged');
  assert.ok(warnings.some(value => /async|Promise/i.test(value)));
  await sleep(0);
});

test('adapter forwards native before/after/persistence envelopes through individual command calls', async () => {
  const { default: plugin } = await import('../../../adapters/openclaw/index.mjs');
  const { mkdtempSync, rmSync } = await import('node:fs');
  const fixtureDir = mkdtempSync(resolve('target/native-lab/openclaw/adapter-test-'));
  try {
    const typedHooks = [];
    plugin.register({
      pluginConfig: { binary: resolve('src/aw/tests/native/openclaw/fixture-client.mjs'), socket: fixtureDir, agent: 'openclaw-lab', hooks: {
        before: [{ provider: 'rewrite', priority: 20 }], after: [{ provider: 'observe', priority: 10 }], persist: [{ provider: 'projection', priority: 30 }],
      } },
      on(hookName, handler, options) { typedHooks.push({ pluginId: plugin.id, hookName, handler, ...options }); },
    });
    assert.equal(typedHooks.length, 3);
    assert.deepEqual(typedHooks.map(item => item.priority), [20, 10, 30]);
    const run = runner(typedHooks);
    assert.equal((await run.runBeforeToolCall(event(), { agentId: 'test' })).params.command, 'node calculation.mjs --aw');
    assert.equal(await run.runAfterToolCall({ ...event(), result: 'observed' }, {}), undefined);
    const result = run.runToolResultPersist({ toolName: 'exec', message: { role: 'toolResult', content: [{ type: 'text', text: 'raw' }] } }, {});
    assert.equal(result.message.content[0].text, 'native-projected-value-73');
    const records = readFileSync(resolve(fixtureDir, 'hooks.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
    assert.deepEqual(records.map(row => row.hook), ['before_tool_call', 'after_tool_call', 'tool_result_persist']);
    assert.equal(records[0].context.agentId, 'test');
  } finally {
    rmSync(fixtureDir, { recursive: true });
  }
});

test('adapter preserves explicit native ask and propagates command failures', async () => {
  const { default: plugin } = await import('../../../adapters/openclaw/index.mjs');
  const { mkdtempSync, rmSync } = await import('node:fs');
  const fixtureDir = mkdtempSync(resolve('target/native-lab/openclaw/adapter-errors-'));
  try {
    for (const provider of ['ask', 'failure']) {
      const typedHooks = [];
      plugin.register({
        pluginConfig: { binary: resolve('src/aw/tests/native/openclaw/fixture-client.mjs'), socket: fixtureDir, agent: 'lab', hooks: { before: [{ provider }] } },
        on(hookName, handler, options) { typedHooks.push({ pluginId: plugin.id, hookName, handler, ...options }); },
      });
      const run = runner(typedHooks);
      if (provider === 'ask') {
        const result = await run.runBeforeToolCall(event(), {});
        assert.equal(result.requireApproval.title, 'AW native approval');
        assert.equal(result.requireApproval.pluginId, plugin.id);
        assert.notEqual(result.block, true);
      } else {
        await assert.rejects(run.runBeforeToolCall(event(), {}), /fixture provider failed/);
      }
    }
  } finally {
    rmSync(fixtureDir, { recursive: true });
  }
});

test('official execution policy distinguishes ask from allow in noninteractive deny/report modes', async () => {
  const { mkdtempSync, rmSync } = await import('node:fs');
  const isolated = mkdtempSync(resolve('target/native-lab/openclaw/approval-policy-'));
  const loadExport = async (prefix, exportName) => {
    const name = readdirSync(resolve(packageRoot, 'dist')).find(name => name.startsWith(prefix) && name.endsWith('.mjs') && readFileSync(resolve(packageRoot, 'dist', name), 'utf8').includes(`${exportName} as `));
    const source = readFileSync(resolve(packageRoot, 'dist', name), 'utf8');
    const alias = source.match(new RegExp(`${exportName} as (\\w+)`))[1];
    return (await import(pathToFileURL(resolve(packageRoot, 'dist', name))))[alias];
  };
  const initialize = await loadExport('hook-runner-global-', 'initializeGlobalHookRunner');
  const reset = await loadExport('hook-runner-global-', 'resetGlobalHookRunner');
  const runPolicy = await loadExport('agent-tools.before-tool-call-', 'runBeforeToolCallHook');
  try {
    const resolutions = [];
    initialize({ hooks: [], plugins: [], typedHooks: [hook('before_tool_call', 'ask-native', 100, () => ({
      requireApproval: { title: 'Native ask', description: 'Approval required by AW', onResolution: result => resolutions.push(result) },
    }))] });
    const denied = await runPolicy({ toolName: 'exec', params: { command: 'printf harmless' }, ctx: { config: {} }, approvalMode: 'deny' });
    assert.equal(denied.blocked, true);
    assert.equal(denied.deniedReason, 'plugin-approval');
    assert.equal(denied.reason, 'approval_required');
    const report = await runPolicy({ toolName: 'exec', params: { command: 'printf harmless' }, ctx: { config: {} }, approvalMode: 'report' });
    assert.equal(report.blocked, true);
    assert.equal(report.reason, 'Approval required by AW');
    await sleep(0);
    assert.deepEqual(resolutions, ['deny', 'cancelled']);
  } finally {
    reset();
    rmSync(isolated, { recursive: true });
  }
});

test('official result middleware awaits registration order and exposes the chained result', async () => {
  const name = readdirSync(resolve(packageRoot, 'dist')).find(name => /^tool-result-middleware-.*\.mjs$/.test(name));
  const source = readFileSync(resolve(packageRoot, 'dist', name), 'utf8');
  const alias = source.match(/createAgentToolResultMiddlewareRunner as (\w+)/)[1];
  const create = (await import(pathToFileURL(resolve(packageRoot, 'dist', name))))[alias];
  const order = [];
  const run = create({ runtime: 'openclaw' }, [
    async event => {
      order.push('first-start');
      await sleep(5);
      order.push('first-end');
      return { result: { ...event.result, content: [{ type: 'text', text: 'first' }] } };
    },
    event => {
      order.push(event.result.content[0].text);
      return { result: { ...event.result, content: [{ type: 'text', text: 'second' }] } };
    },
  ]);
  const result = await run.applyToolResultMiddleware({ toolName: 'exec', toolCallId: 'lab', args: {}, result: { content: [{ type: 'text', text: 'raw' }] } });
  assert.equal(result.content[0].text, 'second');
  assert.deepEqual(order, ['first-start', 'first-end', 'first']);
  const failed = create({ runtime: 'openclaw' }, [() => { throw new Error('provider unavailable'); }]);
  const failure = await failed.applyToolResultMiddleware({ toolName: 'exec', toolCallId: 'lab', args: {}, result: { content: [{ type: 'text', text: 'RAW_MUST_NOT_LEAK' }] } });
  assert.ok(!JSON.stringify(failure).includes('RAW_MUST_NOT_LEAK'));
});

test('adapter registers result commands as native middleware without fabricated priority', async () => {
  const { default: plugin } = await import('../../../adapters/openclaw/index.mjs');
  const { mkdtempSync, rmSync } = await import('node:fs');
  const fixtureDir = mkdtempSync(resolve('target/native-lab/openclaw/result-adapter-'));
  try {
    const registrations = [];
    plugin.register({ pluginConfig: { binary: resolve('src/aw/tests/native/openclaw/fixture-client.mjs'), socket: fixtureDir, agent: 'lab', hooks: { result: [{ provider: 'result', matcher: ['exec'] }, { provider: 'failure' }] } },
      on() { throw new Error('Result middleware must not become an after hook'); },
      registerAgentToolResultMiddleware(handler, options) { registrations.push({ handler, options }); },
    });
    assert.deepEqual(registrations[0].options, { runtimes: ['openclaw'], matcher: ['exec'] });
    const result = await registrations[0].handler({ toolName: 'exec', toolCallId: 'lab', args: {}, result: { content: [{ type: 'text', text: 'raw' }] } }, { runtime: 'openclaw' });
    assert.equal(result.result.content[0].text, 'native-projected-value-73');
    await assert.rejects(registrations[1].handler({ toolName: 'exec', toolCallId: 'lab', args: {}, result: { content: [{ type: 'text', text: 'raw' }] } }, { runtime: 'openclaw' }), /fixture provider failed/);
  } finally {
    rmSync(fixtureDir, { recursive: true });
  }
});


test('native command callbacks inherit a custom environment value and host cwd', async () => {
  const { default: plugin } = await import('../../../adapters/openclaw/index.mjs');
  const { mkdtempSync, rmSync } = await import('node:fs');
  const fixtureDir = mkdtempSync(resolve('target/native-lab/openclaw/environment-adapter-'));
  const previous = process.env.AW_NATIVE_TEST_SENTINEL;
  process.env.AW_NATIVE_TEST_SENTINEL = 'aw-nonsecret-environment-sentinel';
  try {
    const typedHooks = [];
    const middleware = [];
    plugin.register({
      pluginConfig: { binary: resolve('src/aw/tests/native/openclaw/fixture-client.mjs'), socket: fixtureDir, agent: 'lab', hooks: {
        before: [{ provider: 'environment' }], persist: [{ provider: 'environment' }], result: [{ provider: 'environment' }],
      } },
      on(hookName, handler, options) { typedHooks.push({ pluginId: plugin.id, hookName, handler, ...options }); },
      registerAgentToolResultMiddleware(handler) { middleware.push(handler); },
    });
    const expected = { sentinel: 'aw-nonsecret-environment-sentinel', cwd: process.cwd() };
    const run = runner(typedHooks);
    const before = await run.runBeforeToolCall(event(), {});
    assert.equal(before.params.sentinel, expected.sentinel);
    assert.equal(before.params.cwd, expected.cwd);
    const persisted = run.runToolResultPersist({ toolName: 'exec', message: { role: 'toolResult', content: [] } }, {});
    assert.deepEqual(JSON.parse(persisted.message.content[0].text), expected);
    const result = await middleware[0]({ toolName: 'exec', toolCallId: 'lab', args: {}, result: { content: [] } }, { runtime: 'openclaw' });
    assert.deepEqual(JSON.parse(result.result.content[0].text), expected);
    const records = readFileSync(resolve(fixtureDir, 'hooks.jsonl'), 'utf8');
    assert.ok(!records.includes(expected.sentinel), 'Fixture logs contain native payloads, never inherited environment');
  } finally {
    if (previous === undefined) delete process.env.AW_NATIVE_TEST_SENTINEL;
    else process.env.AW_NATIVE_TEST_SENTINEL = previous;
    rmSync(fixtureDir, { recursive: true });
  }
});


test('adapter accepts admitted output larger than one MiB in async and sync hooks', async () => {
  const { default: plugin } = await import('../../../adapters/openclaw/index.mjs');
  const { mkdtempSync, rmSync } = await import('node:fs');
  const fixtureDir = mkdtempSync(resolve('target/native-lab/openclaw/output-budget-'));
  try {
    const handlers = [];
    plugin.register({ pluginConfig: { binary: resolve('src/aw/tests/native/openclaw/fixture-client.mjs'), socket: fixtureDir, agent: 'lab', hooks: {
      before: [{ provider: 'large-output' }], persist: [{ provider: 'large-output' }],
    } }, on(_name, handler) { handlers.push(handler); } });
    for (const handler of handlers) {
      assert.equal((await handler(event(), {})).payload.length, 2 * 1024 * 1024);
    }
  } finally { rmSync(fixtureDir, { recursive: true }); }
});
