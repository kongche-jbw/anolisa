#!/usr/bin/env python3
"""Exercise AW IPC from a real isolated OpenClaw Gateway and model turn."""
import argparse
import json
from pathlib import Path
import subprocess
import time

parser = argparse.ArgumentParser()
parser.add_argument('--key-file', type=Path, required=True)
parser.add_argument('--name', default='live-aw')
parser.add_argument('--launcher', action='store_true')
parser.add_argument('--mode', choices=['projection', 'deny', 'ask'], default='projection')
args = parser.parse_args()
repo = Path.cwd()
lab = repo / 'target/native-lab/openclaw'
run = lab / args.name
run.mkdir(exist_ok=True)
socket = run / 'aw' / 'aw.sock'
if len(str(socket).encode()) >= 108:
    raise ValueError('Choose a shorter run name for the Unix socket path')
node = lab / 'runtime/node_modules/node/bin/node'
aw = repo / 'src/aw/target/debug/aw'
fixture = repo / 'src/aw/tests/native/openclaw/fixture-client.mjs'
providers = ['rewrite' if args.mode == 'projection' else args.mode, 'observe']
if args.mode == 'projection':
    providers += ['result', 'persist-observe']
steps = {
    'tool.before': [{'id': providers[0], 'provider': providers[0], 'native': {'priority': 100}}],
    'tool.after': [{'id': 'observe', 'provider': 'observe', 'native': {'priority': 100}}],
}
if args.mode == 'projection':
    steps['tool.after'] += [
        {'id': 'result', 'provider': 'result', 'native': {'point': 'agent_tool_result'}},
        {'id': 'persist-observe', 'provider': 'persist-observe', 'native': {'point': 'tool_result_persist', 'priority': 100}},
    ]
config = {'apiVersion': 'aw/v1alpha1', 'kind': 'AWConfiguration', 'metadata': {'name': 'openclaw-native-lab'}, 'spec': {
    'daemon': {'startup': 'on_demand', 'endpoint': 'auto', 'state_dir': 'auto'},
    'execution': {'guarantee': 'native_hook', 'default_event_budget_ms': 10000},
    'audit': {'enabled': True, 'payload': 'metadata_only'},
    'agents': {'openclaw-lab': {'adapter': 'openclaw', 'argv': [str(node), str(lab / 'runtime/node_modules/openclaw/openclaw.mjs'), 'gateway', 'run']}},
    'providers': {provider: {'protocol': 'native-hook/v1alpha1', 'transport': {'type': 'stdio', 'location': 'agent', 'argv': [str(node), str(fixture), '--socket', str(run), '--provider', provider]}, 'timeout_ms': 3000, 'max_output_bytes': 1048576, 'config': {}} for provider in providers},
    'events': {event: {'enabled': True, 'required': True, 'steps': entries} for event, entries in steps.items()},
}}
config_path = run / 'aw.json'
config_path.write_text(json.dumps(config, indent=2) + '\n')
subprocess.run([str(aw), 'validate', '--config', str(config_path)], check=True, timeout=10)
bounded = repo / 'src/aw/tests/native/openclaw/run-bounded.py'
child = subprocess.Popen(['python3', str(bounded), '--root', str(lab), '--name', args.name + '-daemon', '--timeout', '210', '--', str(aw), 'serve', '--config', str(config_path), '--socket', str(socket), '--idle-timeout', '120'])
try:
    for _ in range(50):
        if child.poll() is not None:
            raise RuntimeError('AW daemon exited before readiness')
        if socket.exists():
            break
        time.sleep(0.1)
    else:
        raise TimeoutError('AW daemon did not become ready within five seconds')
    subprocess.run(['python3', str(repo / 'src/aw/tests/native/openclaw/run-live.py'), '--key-file', str(args.key_file), '--gateway', '--name', args.name, '--mode', args.mode, '--aw-binary', str(aw), '--aw-socket', str(socket), *(['--launcher'] if args.launcher else [])], check=True, timeout=190)
finally:
    if child.poll() is None:
        subprocess.run([str(aw), 'stop', '--socket', str(socket)], check=False, timeout=10)
        child.wait(timeout=15)
    if socket.exists():
        raise RuntimeError('Owned AW socket was not removed on shutdown')

subprocess.run(['python3', str(repo / 'src/aw/tests/native/openclaw/verify-live.py'), '--name', args.name, '--mode', args.mode, *(['--launcher'] if args.launcher else [])], check=True, timeout=10)
