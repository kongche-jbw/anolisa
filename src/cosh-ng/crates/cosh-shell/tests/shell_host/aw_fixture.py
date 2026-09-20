"""Synthetic Qoder peer; these tests do not certify a real Agent or Herdr UI."""

import ctypes
import hashlib
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unittest
import fcntl

BINARY = Path(sys.argv.pop(1)).resolve()
OWNED_PANES = []
OWNED_TEMPORARIES = []
NATIVE = r'''#!/usr/bin/python3
import json,os,pathlib,shlex,subprocess,sys,termios,fcntl,struct
if sys.argv[1:] == ['--version']:
 print('1.1.47');sys.exit(0)
settings=pathlib.Path(sys.argv[sys.argv.index('--settings')+1])
root=settings.parent
workspace=pathlib.Path.cwd()
hooks=json.loads(settings.read_text())['hooks']
session='session-one'
def hook(kind,session_id=None,tool='tool-one',source='startup'):
 event={'hook_event_name':kind,'session_id':session_id or session,'source':source,
        'cwd':str(workspace),'tool_use_id':tool,'tool_name':'Bash','tool_input':{'command':'echo test'},'tool_response':'test','reason':'prompt_input_exit','prompt':'input','trigger':'manual',
        'compact_summary':'summary','stop_hook_active':False}
 command=shlex.split(hooks[kind][0]['hooks'][0]['command'])
 return subprocess.run(command,input=json.dumps(event),text=True,capture_output=True,timeout=6)
def view():
 helper=shlex.split(hooks['SessionStart'][0]['hooks'][0]['command'])[0]
 return json.loads(subprocess.check_output([helper,'--aw-query',str(root)],timeout=3))
hook('SessionStart')
(workspace/'ready.json').write_text(json.dumps({'pid':os.getpid(),'root':str(root),'tty':os.isatty(0),'argv':sys.argv}))
count=0
while count<20:
 data=b''
 while len(data)<1024:
  byte=os.read(0,1)
  if not byte: sys.exit(0)
  if byte==b'\x03': sys.exit(130)
  if byte in (b'\r',b'\n'): break
  data+=byte
 command=data.decode()
 if not command: continue
 count+=1
 if command=='call':
  hook('PreToolUse',tool=f'tool-{count}')
  hook('PostToolUse',tool=f'tool-{count}')
 elif command=='lifecycle':
  for kind in ('UserPromptSubmit','PreCompact','PostCompact','Stop'):hook(kind)
 elif command=='reset':
  hook('SessionEnd');session='session-two';hook('SessionStart',source='clear')
 elif command=='late':
  hook('PostToolUse','session-one',tool='tool-1')
 elif command=='gap':
  hook('PostToolUse',tool='missing-pretool')
 elif command=='quit':
  hook('SessionEnd');sys.exit(7)
 result=view()
 result['winsize']=list(struct.unpack('HHHH',fcntl.ioctl(0,termios.TIOCGWINSZ,b'\0'*8))[:2])
 (workspace/f'result-{count}.json').write_text(json.dumps(result))
sys.exit(9)
'''

HANDLER = r'''#!/usr/bin/python3
import json,sys,pathlib
event=json.load(sys.stdin)
assert event['event']=='tool.result_observed'
assert 'tool_response' not in event
pathlib.Path('handler-called').write_text(event['session_id'])
print('{"format":1,"observed":true}')
'''


def wait_for(predicate, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.02)
    raise AssertionError('bounded fixture wait expired')


def descendants(pid):
    found = []
    pending = [pid]
    for _ in range(256):
        if not pending:
            return found
        current = pending.pop()
        try:
            stat = Path(f'/proc/{current}/stat').read_text().rsplit(')', 1)[1].split()
            found.append((current, stat[19]))
            pending.extend(int(v) for v in Path(f'/proc/{current}/task/{current}/children').read_text().split())
        except FileNotFoundError:
            pass
    raise AssertionError('owned fixture tree exceeded bound')


class Pane:
    def __init__(self, root, required=False, version='1.1.47', slow_probe=False, lifecycle=False, guard=False, input_response=False, stop_response=False, owner=False, fast_exit=False, owner_failure=False, owner_slow=False):
        self.root = root
        root.mkdir()
        self.home = root / 'home'
        self.home.mkdir()
        native = root / 'native'
        native.mkdir()
        self.native_settings = native / 'settings.json'
        self.native_settings.write_text('{"theme":"dark","hooks":{"Stop":[]}}')
        qoder = root / 'native-qoder'
        native_source = NATIVE.replace("print('1.1.47')", f"print({version!r})")
        if slow_probe:
            native_source = native_source.replace(
                f"print({version!r})", "pathlib.Path('probe.json').write_text(json.dumps("
                "{'pid':os.getpid(),'owner':os.getppid()}));import time;time.sleep(10);"
                f"print({version!r})")
        if fast_exit:
            native_source = native_source.replace("hook('SessionStart')", "# No native callbacks")
            native_source = native_source.replace('count=0', 'sys.exit(9)\ncount=0')
        qoder.write_text(native_source)
        qoder.chmod(0o700)
        handler = root / 'observer'
        handler.write_text(HANDLER.replace("assert event['event']=='tool.result_observed'",
                           "assert event['format']==1 and event['source']=='native_callback'") if lifecycle else HANDLER)
        handler.chmod(0o700)

        def pin(program):
            return dict(provider_id='observer', provider_version='1', program=str(program),
                        program_sha256=hashlib.sha256(program.read_bytes()).hexdigest(),
                        cwd=str(root), args=[], environment={}, pins=[],
                        limits=dict(timeout_ms=1000, input_bytes=65536, output_bytes=1024, stderr_bytes=1024))

        config = root / 'aw.json'
        value = dict(format=1, required_safety=required,
                     qoder={k:v for k,v in pin(qoder).items() if k in ('program','program_sha256')},
                     handler=pin(handler), native_config_directory=str(native))
        if lifecycle:
            command = value.pop('handler')
            value.update(format=2, cwd=str(root), notifications={event:[command] for event in (
                'session.start','input.submit','tool.before','tool.after','compact.before',
                'compact.after','turn.stop','session.end')})
        if guard:
            value.pop('handler', None)
            value.update(format=2, cwd=str(root), notifications={},
                         tool_guard=dict(transforms=[], scanner=pin(handler)))
        if input_response:
            value.pop('handler', None)
            handler.write_text("#!/usr/bin/python3\nprint('{\"format\":1,\"decision\":\"continue\"}')\n")
            value.update(format=2, cwd=str(root), notifications={}, input_response=pin(handler))
        if stop_response:
            value.pop('handler', None)
            handler.write_text("#!/usr/bin/python3\nprint('{\"format\":1,\"decision\":\"allow_stop\"}')\n")
            value.update(format=2, cwd=str(root), notifications={}, stop_response=pin(handler))
        if owner:
            handler.write_text("#!/usr/bin/python3\nimport json,sys,pathlib\n"
                               "e=json.load(sys.stdin)\nassert e['source']=='runtime_owner'\n"
                               "with pathlib.Path('owner-events.jsonl').open('a') as f:f.write(json.dumps(e)+'\\n')\n"
                               + ("sys.exit(1)\n" if owner_failure else "print('{\"format\":1,\"observed\":true}')\n"))
            if owner_slow:
                handler.write_text("#!/usr/bin/python3\nimport os,pathlib,time\n"
                                   "pathlib.Path('owner-handler-pid').write_text(str(os.getpid()))\n"
                                   "time.sleep(10)\n")
            value.pop('handler', None)
            value.update(format=2, cwd=str(root), notifications={event:[pin(handler)] for event in (
                'runtime.observed', 'runtime.exited', 'coverage.changed')})
        config.write_text(json.dumps(value))
        config.chmod(0o600)
        environment = {**os.environ, 'HOME': str(self.home), 'TMPDIR': str(root), 'COSH_AW_CONFIG': str(config),
                       'COSH_AW_CONFIG_SHA256': hashlib.sha256(config.read_bytes()).hexdigest(),
                       'COSH_SHELL_INTEGRATION': 'enhanced', 'COSH_POC_PS1': 'AWTEST> ',
                       'COSH_RECOMMENDATIONS_ENABLED': '0', 'PYTHONDONTWRITEBYTECODE': '1'}
        environment.pop('HERDR_SOCKET_PATH', None)
        environment.pop('HERDR_PANE_ID', None)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(root)
            os.execve(BINARY, [str(BINARY), 'raw', 'fake', '--isolated'], environment)
        os.set_blocking(self.fd, False)
        self.output = bytearray()
        self.sequence = 0
        self.scope = None
        self.reaped = False
        OWNED_PANES.append(self)

    def drain(self):
        for _ in range(32):
            if not select.select([self.fd], [], [], 0)[0]:
                break
            try:
                data = os.read(self.fd, 65536)
            except OSError:
                break
            if not data:
                break
            self.output.extend(data)
        return self.output

    def send(self, text):
        os.write(self.fd, text.encode())

    def launch(self, command='qoder'):
        wait_for(lambda: b'AWTEST>' in self.drain())
        self.send(command + '; printf "NATIVE_EXIT=%s\\n" "$?"\n')
        wait_for(lambda: (self.drain(), (self.root / 'ready.json').exists())[1])
        ready = json.loads((self.root / 'ready.json').read_text())
        self.scope = Path(ready['root']).parent
        return ready

    def action(self, command):
        self.sequence += 1
        self.send(command + '\n')
        result = self.root / f'result-{self.sequence}.json'
        wait_for(lambda: (self.drain(), result.exists())[1])
        return json.loads(result.read_text())

    def close(self):
        if self.reaped:
            return
        owned = descendants(self.pid)
        for pid, ticks in reversed(owned):
            try:
                current = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[19]
                if current == ticks:
                    os.kill(pid, signal.SIGKILL)
            except (FileNotFoundError, ProcessLookupError):
                pass
        deadline = time.monotonic() + 3
        pending = {pid for pid, _ in owned}
        while pending and time.monotonic() < deadline:
            for pid in list(pending):
                try:
                    if os.waitpid(pid, os.WNOHANG)[0]:
                        pending.remove(pid)
                except ChildProcessError:
                    if not Path(f'/proc/{pid}').exists():
                        pending.remove(pid)
            time.sleep(0.01)
        os.close(self.fd)
        self.reaped = True
        assert not pending, f'fixture children remain: {pending}'
        # SIGKILL bypasses Rust destructors; remove only this fixture's known scope.
        if self.scope and self.scope.parent.exists():
            import shutil
            shutil.rmtree(self.scope.parent)

    def finish(self):
        self.send('quit\n')
        wait_for(lambda: b'NATIVE_EXIT=7' in self.drain())
        self.finish_shell()

    def finish_shell(self):
        self.send('exit\n')
        wait_for(lambda: (self.drain(), os.waitpid(self.pid, os.WNOHANG)[0])[1])
        self.reaped = True
        os.close(self.fd)
        assert not self.scope.exists(), 'shell-scoped AW resources survived normal exit'


class InteractivePty(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='aw-pty-', dir=BINARY.parent)
        OWNED_TEMPORARIES.append(self.temporary)
        self.panes = []

    def tearDown(self):
        try:
            for pane in self.panes:
                pane.close()
        finally:
            self.temporary.cleanup()

    def pane(self, **options):
        pane = Pane(Path(self.temporary.name) / str(len(self.panes)), **options)
        self.panes.append(pane)
        return pane

    def test_persistent_pty_reset_resize_exit_and_native_settings(self):
        pane = self.pane()
        settings = pane.native_settings.read_bytes()
        ready = pane.launch()
        self.assertTrue(ready['tty'])
        first = pane.action('call')
        self.assertEqual(first['observed'], 1)
        fcntl.ioctl(pane.fd, termios.TIOCSWINSZ, struct.pack('HHHH', 31, 101, 0, 0))
        reset = pane.action('reset')
        self.assertEqual(reset['attachment'], 2)
        self.assertEqual(reset['agent_pid'], ready['pid'])
        self.assertEqual(reset['winsize'], [31, 101])
        self.assertEqual(pane.action('late')['observed'], 0)
        self.assertEqual(pane.action('call')['observed'], 1)
        self.assertEqual(pane.native_settings.read_bytes(), settings)
        pane.finish()

    def test_lifecycle_configuration_reaches_core_on_the_natural_entry(self):
        pane = self.pane(lifecycle=True)
        original = pane.native_settings.read_bytes()
        ready = pane.launch()
        hooks = json.loads((Path(ready['root']) / 'native.json').read_text())['hooks']
        self.assertEqual(len(hooks), 12)
        self.assertIn('SubagentStop', hooks)
        self.assertEqual(pane.action('call')['observed'], 3)
        report = pane.action('lifecycle')
        self.assertEqual(report['observed'], 7)
        self.assertEqual(report['effect'], 'notify_only')
        journal = Path(ready['root']) / 'notification-journal'
        self.assertEqual(len(list(journal.glob('*.jsonl'))), 7)
        self.assertEqual(pane.native_settings.read_bytes(), original)
        pane.finish()

    def test_two_panes_have_independent_identity_and_cleanup(self):
        first, second = self.pane(), self.pane()
        a, b = first.launch(), second.launch()
        self.assertNotEqual(a['pid'], b['pid'])
        self.assertNotEqual(a['root'], b['root'])
        first.action('call')
        self.assertEqual(second.action('call')['observed'], 1)
        first.finish()
        self.assertEqual(second.action('call')['observed'], 2)
        second.finish()

    def test_guard_entry_blocks_helper_errors_and_is_selected_only_for_pretool(self):
        pane = self.pane(guard=True)
        ready = pane.launch()
        hooks = json.loads((Path(ready['root']) / 'native.json').read_text())['hooks']
        for event, entries in hooks.items():
            self.assertIn(' --aw-guard ' if event == 'PreToolUse' else ' --aw-hook ',
                          entries[0]['hooks'][0]['command'])
        for payload in ('{', '{}'):
            blocked = subprocess.run([BINARY, '--aw-guard', str(pane.root / 'absent')],
                                     input=payload, text=True, capture_output=True, timeout=3)
            self.assertEqual(blocked.returncode, 2)
            self.assertEqual(blocked.stdout, '')
            optional = subprocess.run([BINARY, '--aw-hook', str(pane.root / 'absent')],
                                      input=payload, text=True, capture_output=True, timeout=3)
            self.assertEqual(optional.returncode, 0)
            self.assertIn('systemMessage', json.loads(optional.stdout))
        pane.finish()

    def test_input_entry_is_selected_only_for_submission_and_blocks_parse_errors(self):
        pane = self.pane(input_response=True)
        ready = pane.launch()
        hooks = json.loads((Path(ready['root']) / 'native.json').read_text())['hooks']
        for event, entries in hooks.items():
            self.assertIn(' --aw-input ' if event == 'UserPromptSubmit' else ' --aw-hook ',
                          entries[0]['hooks'][0]['command'])
        for payload in ('{', '{}'):
            blocked = subprocess.run([BINARY, '--aw-input', str(pane.root / 'absent')],
                                     input=payload, text=True, capture_output=True, timeout=3)
            self.assertEqual(blocked.returncode, 2)
            self.assertEqual(blocked.stdout, '')
        report = pane.action('lifecycle')
        self.assertEqual(report['input_response'], 'experimental_native_response')
        self.assertEqual(len(list((Path(ready['root']) / 'input-response-journal').glob('*.jsonl'))), 1)
        pane.finish()

    def test_stop_entry_is_selected_only_for_stop_and_errors_do_not_request_work(self):
        pane = self.pane(stop_response=True)
        ready = pane.launch()
        hooks = json.loads((Path(ready['root']) / 'native.json').read_text())['hooks']
        for event, entries in hooks.items():
            self.assertIn(' --aw-stop ' if event == 'Stop' else ' --aw-hook ',
                          entries[0]['hooks'][0]['command'])
        for payload in ('{', '{}'):
            stopped = subprocess.run([BINARY, '--aw-stop', str(pane.root / 'absent')],
                                     input=payload, text=True, capture_output=True, timeout=3)
            self.assertEqual(stopped.returncode, 0)
            output = json.loads(stopped.stdout)
            self.assertIs(output['continue'], False)
            self.assertIn('systemMessage', output)
            self.assertNotIn('decision', output)
        report = pane.action('lifecycle')
        self.assertEqual(report['stop_response'], 'experimental_native_response')
        self.assertEqual(len(list((Path(ready['root']) / 'stop-response-journal').glob('*.jsonl'))), 1)
        pane.finish()

    def owner_events(self, pane):
        path = pane.root / 'owner-events.jsonl'
        if not path.exists():
            return []
        # A concurrent append may leave only the last line incomplete.
        lines = path.read_text().splitlines(keepends=True)
        return [json.loads(line) for line in lines if line.endswith('\n')]

    def wait_owner(self, pane, event, predicate=lambda value: True):
        return wait_for(lambda: next((e for e in self.owner_events(pane)
                                      if e['event'] == event and predicate(e)), None))

    def test_owner_events_without_viewer_cover_reset_gap_exit_and_readonly_queries(self):
        pane = self.pane(owner=True)
        ready = pane.launch()
        root = Path(ready['root'])
        observed = self.wait_owner(pane, 'runtime.observed')
        self.assertFalse(observed['payload']['agent_ready'])
        self.assertIsNone(observed['session_id'])
        self.wait_owner(pane, 'coverage.changed', lambda e: e['payload']['current']['native_callbacks'] == 'attached')
        for _ in range(2):
            snapshot = json.loads(subprocess.check_output([BINARY, '--aw-query', str(root)], timeout=3))
            self.assertEqual(snapshot['runtime_observer']['status'], 'active')
        self.assertEqual(sum(e['event'] == 'runtime.observed' for e in self.owner_events(pane)), 1)
        pane.action('reset')
        self.wait_owner(pane, 'coverage.changed', lambda e: e['payload']['current']['session_epoch'] == 2)
        pane.action('gap')
        self.wait_owner(pane, 'coverage.changed', lambda e: e['payload']['current']['native_callbacks'] == 'gap')
        pane.send('quit\n')
        exited = self.wait_owner(pane, 'runtime.exited')
        self.assertEqual(exited['session_id'], 'session-two')
        self.assertIsNone(exited['payload']['exit_status'])
        self.assertFalse(exited['payload']['descendants_reaped'])
        self.wait_owner(pane, 'coverage.changed', lambda e: e['payload']['current']['runtime'] == 'exited')
        wait_for(lambda: b'NATIVE_EXIT=7' in pane.drain())
        snapshot = json.loads(subprocess.check_output([BINARY, '--aw-query', str(root)], timeout=3))
        self.assertTrue(snapshot['owner_observation']['root_exited'])
        self.assertEqual(sum(e['event'] == 'runtime.exited' for e in self.owner_events(pane)), 1)
        pane.finish_shell()

    def test_pidfd_sees_killed_runtime_without_session_end_and_keeps_panes_isolated(self):
        first, second = self.pane(owner=True), self.pane(owner=True)
        a, b = first.launch(), second.launch()
        self.wait_owner(first, 'runtime.observed')
        self.wait_owner(second, 'runtime.observed')
        os.kill(a['pid'], signal.SIGKILL)
        event = self.wait_owner(first, 'runtime.exited')
        self.assertEqual(event['payload']['source'], 'pidfd')
        self.assertTrue(Path('/proc', str(b['pid'])).exists())
        self.assertFalse(any(e['event'] == 'runtime.exited' for e in self.owner_events(second)))
        wait_for(lambda: b'NATIVE_EXIT=137' in first.drain())
        first.finish_shell()
        second.send('quit\n')
        self.wait_owner(second, 'runtime.exited')
        second.finish_shell()

    def test_owner_captures_immediate_exit_and_bounds_failed_coverage_delivery(self):
        pane = self.pane(owner=True, fast_exit=True, owner_failure=True)
        pane.launch()
        event = self.wait_owner(pane, 'runtime.exited')
        self.assertIsNone(event['session_id'])
        self.wait_owner(pane, 'coverage.changed', lambda e: e['payload']['current']['runtime'] == 'exited')
        events = self.owner_events(pane)
        self.assertEqual(sum(e['event'] == 'runtime.observed' for e in events), 1)
        self.assertEqual(sum(e['event'] == 'runtime.exited' for e in events), 1)
        self.assertLessEqual(sum(e['event'] == 'coverage.changed' for e in events), 3)
        self.assertTrue(all(e['payload']['current']['os_coverage'] == 'not_attached'
                            for e in events if e['event'] == 'coverage.changed'))
        wait_for(lambda: b'NATIVE_EXIT=9' in pane.drain())
        pane.finish_shell()

    def test_owner_teardown_cancels_and_reaps_its_handler(self):
        pane = self.pane(owner=True, owner_slow=True)
        pane.launch()
        path = pane.root / 'owner-handler-pid'
        wait_for(path.exists)
        pid = int(path.read_text())
        pane.send('quit\n')
        wait_for(lambda: b'NATIVE_EXIT=7' in pane.drain())
        pane.finish_shell()
        self.assertFalse(Path('/proc', str(pid)).exists())

    def test_required_security_refuses_before_native_start(self):
        pane = self.pane(required=True)
        wait_for(lambda: b'required safety unavailable' in pane.drain())
        self.assertFalse((pane.root / 'ready.json').exists())

    def test_ctrl_c_reaches_native_foreground_and_shell_recovers(self):
        pane = self.pane()
        ready = pane.launch('qoder --model auto "two words"')
        self.assertEqual(ready['argv'][-3:], ['--model', 'auto', 'two words'])
        pane.send('\x03')
        # Bash can abandon the remainder of the interrupted command list.
        wait_for(lambda: (pane.drain(), not Path(f"/proc/{ready['pid']}").exists())[1])
        pane.send('printf "INTERRUPT_STATUS=%s\\n" "$?"\n')
        wait_for(lambda: b'INTERRUPT_STATUS=130' in pane.drain())
        pane.finish_shell()

    def test_version_mismatch_refuses_agent_and_leaves_no_run(self):
        pane = self.pane(version='0.0.0')
        wait_for(lambda: b'AWTEST>' in pane.drain())
        pane.send('qoder\n')
        wait_for(lambda: b'Qoder 1.1.47 required' in pane.drain())
        pane.send('printf "%s" "$COSH_AW_ROOT" > scope-path\n')
        wait_for(lambda: (pane.drain(), (pane.root / 'scope-path').exists())[1])
        pane.scope = Path((pane.root / 'scope-path').read_text())
        self.assertEqual(list(pane.scope.glob('run-*')), [])
        self.assertFalse((pane.root / 'ready.json').exists())
        pane.finish_shell()

    def test_cancelled_startup_reaps_probe_and_leaves_no_run(self):
        pane = self.pane(slow_probe=True)
        wait_for(lambda: b'AWTEST>' in pane.drain())
        pane.send('qoder\n')
        wait_for(lambda: (pane.drain(), (pane.root / 'probe.json').exists())[1])
        probe = json.loads((pane.root / 'probe.json').read_text())
        os.kill(probe['owner'], signal.SIGTERM)
        wait_for(lambda: (pane.drain(), not Path(f"/proc/{probe['pid']}").exists())[1], timeout=3)
        pane.send('printf "%s" "$COSH_AW_ROOT" > scope-path\n')
        wait_for(lambda: (pane.drain(), (pane.root / 'scope-path').exists())[1])
        pane.scope = Path((pane.root / 'scope-path').read_text())
        self.assertEqual(list(pane.scope.glob('run-*')), [])
        self.assertFalse((pane.root / 'ready.json').exists())
        pane.finish_shell()


if __name__ == '__main__':
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise RuntimeError('fixture subreaper unavailable')
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt('fixture interrupted')

    signal.signal(signal.SIGTERM, interrupted)
    try:
        unittest.main()
    finally:
        for pane in OWNED_PANES:
            pane.close()
        for directory in OWNED_TEMPORARIES:
            directory.cleanup()
