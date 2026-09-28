import { execFile, spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { renameSync, rmSync, writeFileSync } from 'node:fs';

function registerReadyReceipt(api, hooks) {
  const file = process.env.AW_READY_FILE;
  const token = process.env.AW_READY_TOKEN;
  if (file === undefined && token === undefined) return;
  if (!file || !token) throw new Error('AW requires both readiness file and token');
  // Discovery can register callbacks without activating their registry. Only the
  // Gateway startup callback attests that this registry reached the live host.
  api.on('gateway_start', () => {
    const temporary = `${file}.${process.pid}.${randomUUID()}.tmp`;
    try {
      writeFileSync(temporary, JSON.stringify({ version: 1, adapter: 'openclaw', pid: process.pid, token, hooks }) + '\n', { mode: 0o600, flag: 'wx' });
      renameSync(temporary, file);
    } finally {
      rmSync(temporary, { force: true });
    }
  });
}

function request(api, handler, hook, event, context) {
  const socket = api.pluginConfig?.socket ?? process.env.AW_SOCKET;
  const agent = api.pluginConfig?.agent ?? process.env.AW_AGENT;
  const binary = api.pluginConfig?.binary ?? process.env.AW_BIN;
  if (!socket || !agent || !binary) {
    throw new Error('AW requires socket, agent, and binary configuration');
  }
  return {
    binary,
    args: ['hook', '--socket', socket, '--agent', agent, '--event', hook === 'before_tool_call' ? 'tool.before' : 'tool.after', '--provider', handler.provider, ...(handler.onError ? ['--adapter', 'openclaw', '--on-error', handler.onError] : [])],
    input: JSON.stringify({ hook, event, context }),
    options: {
      timeout: api.pluginConfig?.timeoutMs ?? 10000,
      maxBuffer: 4 * 1024 * 1024,
      // Native callbacks inherit the host environment and working directory.
      env: process.env,
    },
  };
}

function parseOutput(stdout) {
  return stdout.trim() ? JSON.parse(stdout) : undefined;
}

function invoke(api, handler, hook, event, context) {
  const call = request(api, handler, hook, event, context);
  return new Promise((resolve, reject) => {
    const child = execFile(call.binary, call.args, call.options, (error, stdout) => {
      if (error) {
        reject(error);
        return;
      }
      try {
        resolve(parseOutput(stdout));
      } catch (error) {
        reject(error);
      }
    });
    child.stdin.on('error', reject);
    child.stdin.end(call.input);
  });
}

function invokeSync(api, handler, hook, event, context) {
  const call = request(api, handler, hook, event, context);
  const result = spawnSync(call.binary, call.args, { ...call.options, input: call.input, encoding: 'utf8' });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`AW provider ${handler.provider} exited ${result.status}: ${result.stderr.trim()}`);
  }
  return parseOutput(result.stdout);
}

export default {
  id: 'aw-native-hooks',
  name: 'AW native tool hooks',
  register(api) {
    const hooks = api.pluginConfig?.hooks ?? {};
    let registered = 0;
    for (const [group, hook] of [['before', 'before_tool_call'], ['after', 'after_tool_call']]) {
      for (const handler of hooks[group] ?? []) {
        api.on(hook, (event, context) => invoke(api, handler, hook, event, context), {
          priority: handler.priority ?? 100,
          ...(handler.matcher ? { matcher: handler.matcher } : {}),
        });
        registered += 1;
      }
    }
    for (const handler of hooks.result ?? []) {
      api.registerAgentToolResultMiddleware(
        (event, context) => invoke(api, handler, 'agent_tool_result', event, context),
        { runtimes: ['openclaw'], ...(handler.matcher ? { matcher: handler.matcher } : {}) },
      );
      registered += 1;
    }
    // Persistence is a distinct synchronous native boundary, not after_tool_call.
    for (const handler of hooks.persist ?? []) {
      api.on('tool_result_persist', (event, context) => invokeSync(api, handler, 'tool_result_persist', event, context), {
        priority: handler.priority ?? 100,
      });
      registered += 1;
    }
    registerReadyReceipt(api, registered);
  },
};
