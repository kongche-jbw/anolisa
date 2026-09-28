#!/usr/bin/env python3
"""Verify retained live native evidence without making another model request."""
import argparse
import json
import os
from pathlib import Path
import socket

parser = argparse.ArgumentParser()
parser.add_argument('--name', required=True)
parser.add_argument('--mode', choices=['projection', 'deny', 'ask'], default='projection')
parser.add_argument('--launcher', action='store_true')
args = parser.parse_args()
lab = Path.cwd() / 'target/native-lab/openclaw'
run = lab / args.name
providers = ['rewrite', 'observe', 'result', 'persist-observe'] if args.mode == 'projection' else [args.mode, 'observe']
answer = json.loads((lab / (args.name + '-agent.log')).read_text())
hooks = [json.loads(line) for line in (run / 'hooks.jsonl').read_text().splitlines()]
audit = [json.loads(line) for line in (run / 'aw/audit.jsonl').read_text().splitlines()]
assert answer['status'] == 'ok', 'The real native agent must complete'
final = '\n'.join(row.get('text', '') for row in answer['result']['payloads'])
if args.mode == 'projection':
    by_provider = {row['provider']: row for row in hooks}
    assert by_provider['rewrite']['event']['params']['command'] == 'node calculation.mjs'
    assert by_provider['result']['event']['args']['command'] == 'node calculation.mjs --aw'
    assert by_provider['result']['event']['result']['content'][0]['text'] == 'native-before-value-52'
    assert by_provider['persist-observe']['event']['message']['content'][0]['text'] == 'native-projected-value-73'
    assert by_provider['observe']['event']['result']['content'][0]['text'] == 'native-projected-value-73'
    assert 'native-projected-value-73' in final, 'Model must adopt native result middleware output'
    assert {row['provider'] for row in audit} == set(providers)
    assert all(row['exit_code'] == 0 for row in audit)
evidence = {'openclaw': '2026.9.6', 'node': '24.16.0', 'model': 'qwen3.7-plus',
            'endpoint': 'https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1',
            'through_aw_launcher': args.launcher, 'run_id': answer['runId'],
            'native_hook_sequence': [row['hook'] for row in hooks],
            'aw_audit_providers': [row['provider'] for row in audit],
            'model_final': final, 'assertions': 'passed'}
rows = json.loads((lab / 'processes.json').read_text())
for row in rows:
    if row['name'] in [args.name + '-agent', args.name + '-gateway', args.name + '-daemon']:
        try:
            os.kill(row['pid'], 0)
        except ProcessLookupError:
            pass
        else:
            raise AssertionError(f"Owned process {row['pid']} is still alive")
        for port in row.get('ports', []):
            with socket.socket() as probe:
                assert probe.connect_ex(('127.0.0.1', port)) != 0, f'Port {port} still listening'
assert not (run / 'aw/aw.sock').exists(), 'Owned AW socket must be removed'
evidence['cleanup'] = 'owned agent/Gateway/daemon PIDs absent, ports closed, AW socket removed'
(run / 'evidence.json').write_text(json.dumps(evidence, indent=2) + '\n')
print(json.dumps({'evidence': str(run / 'evidence.json'), 'assertions': 'passed'}))
