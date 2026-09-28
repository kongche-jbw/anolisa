#!/usr/bin/env python3
"""Run one isolated native experiment and retain process ownership evidence."""
import argparse
import json
import fcntl
import os
from pathlib import Path
import signal
import subprocess
import time
import uuid

parser = argparse.ArgumentParser()
parser.add_argument('--root', type=Path, required=True)
parser.add_argument('--name', required=True)
parser.add_argument('--timeout', type=int, required=True)
parser.add_argument('--port', type=int, action='append', default=[])
parser.add_argument('command', nargs=argparse.REMAINDER)
args = parser.parse_args()
command = args.command[1:] if args.command[:1] == ['--'] else args.command
args.root.mkdir(parents=True, exist_ok=True)
log_path = args.root.resolve() / (args.name + '.log')
record_path = args.root / 'processes.json'
record = {'id': str(uuid.uuid4()), 'name': args.name, 'command': command,
          'cwd': os.getcwd(), 'pid': None, 'ports': args.port, 'log': str(log_path),
          'timeout_seconds': args.timeout, 'started': time.time(),
          'stop': None, 'status': 'starting'}


def save_record():
    with record_path.open('a+') as handle:
        fcntl.flock(handle, fcntl.LOCK_EX)
        handle.seek(0)
        records = json.loads(handle.read() or '[]')
        records = [item for item in records if item.get('id') != record['id']]
        records.append(record)
        handle.seek(0)
        handle.truncate()
        handle.write(json.dumps(records, indent=2) + '\n')


termination_signal = None


def terminate_signal(signum, frame):
    global termination_signal
    termination_signal = signum
    raise KeyboardInterrupt()


signal.signal(signal.SIGTERM, terminate_signal)
signal.signal(signal.SIGINT, terminate_signal)
save_record()
with log_path.open('w') as log:
    process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT,
                               start_new_session=True)
    record.update(pid=process.pid, stop=f'kill -TERM -- -{process.pid}', status='running')
    save_record()
    try:
        code = process.wait(timeout=args.timeout)
    except (subprocess.TimeoutExpired, KeyboardInterrupt) as error:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        code = 124 if isinstance(error, subprocess.TimeoutExpired) else 128 + (termination_signal or signal.SIGINT)
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=10)
record.update(status='timed_out' if code == 124 else 'terminated' if termination_signal else 'completed', exit_code=code, ended=time.time())
save_record()
print(json.dumps({'name': args.name, 'exit_code': code, 'log': str(log_path)}))
raise SystemExit(code)
