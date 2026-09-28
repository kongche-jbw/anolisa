#!/usr/bin/env python3
"""Run an official isolated OpenClaw coding turn with a memory-only model key."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import signal
import socket
import time
import uuid

parser = argparse.ArgumentParser()
parser.add_argument('--key-file', type=Path, required=True)
parser.add_argument('--lab-root', type=Path, default=Path('target/native-lab/openclaw'))
parser.add_argument('--name', default='live-fixture')
parser.add_argument('--mode', choices=['projection', 'deny', 'ask'], default='projection')
parser.add_argument('--aw-binary', type=Path)
parser.add_argument('--gateway', action='store_true')
parser.add_argument('--launcher', action='store_true')
parser.add_argument('--aw-socket')
parser.add_argument('--aw-agent', default='openclaw-lab')
args = parser.parse_args()
repo = Path.cwd()
lab = args.lab_root.resolve()
run = lab / args.name
run.mkdir(parents=True, exist_ok=True)
workspace = run / 'workspace'
workspace.mkdir(exist_ok=True)
(run / 'state').mkdir(exist_ok=True)
(run / 'home').mkdir(exist_ok=True)
(workspace / 'calculation.mjs').write_text("console.log(process.argv.includes('--aw') ? 'native-before-value-52' : 2 + 3 + 37);\n")
(workspace / 'AGENTS.md').write_text('This isolated workspace contains a harmless calculation. Use only the requested exec command. Do not inspect environment or parent directories. Stop after at most two attempts.\n')
config = {
    'logging': {'file': str(run / 'openclaw-runtime.log'), 'level': 'info'},
    'models': {'mode': 'merge', 'providers': {'aw-token-plan': {
        'baseUrl': 'https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1',
        'apiKey': '${DASHSCOPE_API_KEY}', 'api': 'openai-completions',
        'models': [{'id': 'qwen3.7-plus', 'name': 'qwen3.7-plus', 'reasoning': False,
                    'input': ['text'], 'contextWindow': 131072, 'maxTokens': 1024}]
    }}},
    'agents': {'defaults': {'model': {'primary': 'aw-token-plan/qwen3.7-plus'}, 'workspace': str(workspace), 'skipBootstrap': True}},
    'tools': {'allow': ['exec'], 'exec': {'mode': 'full'}},
    'plugins': {'allow': ['aw-native-hooks'], 'load': {'paths': [str(repo / 'src/aw/adapters/openclaw')]},
        'entries': {'aw-native-hooks': {'enabled': True, 'config': {
            'binary': str(args.aw_binary.resolve() if args.aw_binary else repo / 'src/aw/tests/native/openclaw/fixture-client.mjs'),
            'socket': args.aw_socket or str(run), 'agent': args.aw_agent,
            'hooks': {
                'before': [{'provider': 'rewrite' if args.mode == 'projection' else args.mode, 'priority': 100}],
                'after': [{'provider': 'observe', 'priority': 100}],
                'result': [{'provider': 'result'}] if args.mode == 'projection' else [],
                'persist': [{'provider': 'persist-observe', 'priority': 100}] if args.mode == 'projection' else []
            }
        }}}},
}
config_path = run / 'openclaw.json'
config_path.write_text(json.dumps(config, indent=2) + '\n')
env = dict(os.environ)
env.update(OPENCLAW_HOME=str(run / 'home'), OPENCLAW_STATE_DIR=str(run / 'state'),
           OPENCLAW_CONFIG_PATH=str(config_path), OPENCLAW_SKIP_CHANNELS='1',
           DASHSCOPE_API_KEY=args.key_file.read_text().strip())
node = lab / 'runtime/node_modules/node/bin/node'
cli = lab / 'runtime/node_modules/openclaw/openclaw.mjs'
command = [str(node), str(cli), 'agent', 'exec', '--config', str(config_path),
           '--state-dir', str(run / 'state'), '--cwd', str(workspace),
           '--model', 'aw-token-plan/qwen3.7-plus', '--code-mode', 'direct', '--thinking', 'off',
           '--timeout', '120', '--json',
           'Use exec exactly once to run node calculation.mjs and report the exact stdout. This is a coding smoke test. Do not read files or change the command. If denied or approval is unavailable, report that and stop; never retry.']
bounded = repo / 'src/aw/tests/native/openclaw/run-bounded.py'
gateway = None
result = None
try:
    if args.gateway:
        with socket.socket() as reservation:
            reservation.bind(('127.0.0.1', 0))
            port = reservation.getsockname()[1]
        config['gateway'] = {'mode': 'local', 'bind': 'loopback', 'port': port,
                             'auth': {'mode': 'token', 'token': uuid.uuid4().hex},
                             'controlUi': {'enabled': False}}
        config['skills'] = {'load': {'watch': False}}
        config['cron'] = {'enabled': False}
        config_path.write_text(json.dumps(config, indent=2) + '\n')
        gateway_command = [str(node), str(cli), 'gateway', 'run', '--bind', 'loopback',
                           '--port', str(port), '--tailscale', 'off']
        if args.launcher:
            if not args.aw_binary:
                raise ValueError('--launcher requires --aw-binary')
            config['plugins'] = {'allow': []}
            config_path.write_text(json.dumps(config, indent=2) + '\n')
            gateway_command = [str(args.aw_binary.resolve()), 'run', args.aw_agent,
                               '--config', str(run / 'aw.json'), '--native-config', str(config_path),
                               '--socket', args.aw_socket, '--state-dir', str(run / 'launch')]
        gateway = subprocess.Popen(['python3', str(bounded), '--root', str(lab),
                                    '--name', args.name + '-gateway', '--timeout', '180',
                                    '--port', str(port), '--', *gateway_command], env=env)
        ready = False
        for _ in range(60):
            if gateway.poll() is not None:
                raise RuntimeError('Isolated Gateway exited before readiness')
            try:
                with socket.create_connection(('127.0.0.1', port), timeout=0.2):
                    ready = True
                    break
            except OSError:
                time.sleep(0.5)
        if not ready:
            raise TimeoutError('Isolated Gateway did not become ready within 30 seconds')
        command = [str(node), str(cli), 'agent', '--session-id', str(uuid.uuid4()),
                   '--model', 'aw-token-plan/qwen3.7-plus', '--thinking', 'off',
                   '--timeout', '120', '--json', '--message', command[-1]]
    result = subprocess.run(['python3', str(bounded), '--root', str(lab), '--name',
                             args.name + '-agent', '--timeout', '140', '--', *command],
                            env=env, timeout=155)
finally:
    cleanup_error = None
    if gateway is not None:
        gateway.send_signal(signal.SIGTERM)
        gateway.wait(timeout=15)
        for _ in range(50):
            with socket.socket() as probe:
                if probe.connect_ex(('127.0.0.1', port)) != 0:
                    break
            time.sleep(0.1)
        else:
            cleanup_error = RuntimeError(f'Owned Gateway port {port} is still listening after five seconds')
    # Never print the credential, including when checking runtime persistence.
    key = env['DASHSCOPE_API_KEY'].encode()
    retained = []
    logs = [lab / (args.name + suffix) for suffix in ['-agent.log', '-gateway.log']]
    for path in [*run.rglob('*'), *logs]:
        if path.is_file() and key in path.read_bytes():
            retained.append(str(path))
            path.write_bytes(path.read_bytes().replace(key, b'REDACTED_MODEL_KEY'))
    if retained:
        print(json.dumps({'credential_files_scrubbed': retained}))
    if cleanup_error:
        raise cleanup_error
raise SystemExit(result.returncode if result else 1)
