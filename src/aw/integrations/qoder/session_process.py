"""Linux ownership of one live child tree; never replay stored PID commands."""

import ctypes
import os
from pathlib import Path
import selectors
import signal
import subprocess
import time


class Processes:
    """Own all children of a dedicated, single-threaded CLI until cleanup."""

    def __init__(self):
        self.cancelled = False
        self.previous = {}
        self.subreaper = None

    @staticmethod
    def children():
        return [
            int(pid) for pid in Path(f"/proc/self/task/{os.getpid()}/children").read_text().split()
        ]

    def require_owner(self):
        if len(list(Path("/proc/self/task").iterdir())) != 1 or self.children():
            raise RuntimeError("launcher requires a single thread without existing children")
        if signal.getsignal(signal.SIGCHLD) != signal.SIG_DFL:
            raise RuntimeError("launcher requires default child reaping")

    @staticmethod
    def prctl(option, argument):
        if ctypes.CDLL(None, use_errno=True).prctl(option, argument, 0, 0, 0) != 0:
            raise RuntimeError("cannot establish launcher child ownership")

    def __enter__(self):
        self.require_owner()
        state = ctypes.c_int()
        self.prctl(37, ctypes.byref(state))  # PR_GET_CHILD_SUBREAPER
        self.subreaper = state.value
        self.prctl(36, 1)  # PR_SET_CHILD_SUBREAPER
        try:
            for sig in (signal.SIGTERM, signal.SIGINT):
                self.previous[sig] = signal.signal(sig, self.cancel)
        except BaseException:
            self.__exit__()
            raise
        return self

    def cancel(self, *_args):
        self.cancelled = True

    def __exit__(self, *_args):
        for sig, handler in self.previous.items():
            signal.signal(sig, handler)
        if self.subreaper is not None:
            self.prctl(36, self.subreaper)
            self.subreaper = None

    @staticmethod
    def send(pid, sig, *, group=False):
        try:
            (os.killpg if group else os.kill)(pid, sig)
        except ProcessLookupError:
            pass

    def cleanup(self, child):
        # No wait/reap occurs before the final group signal: the leader reserves
        # its PGID even after an early exit. Direct children likewise retain
        # their PIDs until their last signal, including adopted Provider groups.
        self.send(child.pid, signal.SIGTERM, group=True)
        grace = time.monotonic() + 1
        termed = set()
        while True:
            for pid in self.children():
                if pid not in termed:
                    self.send(pid, signal.SIGTERM)
                    termed.add(pid)
            if time.monotonic() >= grace:
                break
            time.sleep(0.02)
        self.send(child.pid, signal.SIGKILL, group=True)
        deadline = time.monotonic() + 3
        while True:
            children = self.children()
            if not children:
                return
            for pid in children:
                self.send(pid, signal.SIGKILL)
                exited = os.waitid(os.P_PID, pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
                if exited is not None:
                    if pid == child.pid:
                        child.wait(timeout=0)
                    else:
                        os.waitpid(pid, 0)
            if time.monotonic() >= deadline:
                raise RuntimeError("owned descendants did not exit")
            time.sleep(0.02)

    def run(self, argv, cwd, timeout, *, stdout=None, stderr=None):
        """Wait without reaping the leader; all signals precede its reap."""
        return self.execute(argv, cwd, timeout, stdout=stdout, stderr=stderr)[0]

    def execute(self, argv, cwd, timeout, *, stdout=None, stderr=None):
        self.require_owner()
        if self.subreaper is None:
            raise RuntimeError("launcher process ownership is not active")
        if self.cancelled:
            raise RuntimeError("session cancelled")
        output = bytearray()
        sizes = {"stdout": 0, "stderr": 0}
        selector = selectors.DefaultSelector()
        try:
            child = subprocess.Popen(
                argv,
                cwd=cwd,
                stdin=subprocess.DEVNULL,
                stdout=stdout,
                stderr=stderr,
                start_new_session=True,
            )
        except BaseException:
            selector.close()
            raise
        try:
            for name, pipe in (("stdout", child.stdout), ("stderr", child.stderr)):
                if pipe is not None:
                    os.set_blocking(pipe.fileno(), False)
                    selector.register(pipe, selectors.EVENT_READ, name)
            deadline = time.monotonic() + timeout
            while True:
                exited = os.waitid(os.P_PID, child.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
                if self.cancelled:
                    raise RuntimeError("session cancelled")
                if exited is not None and not selector.get_map():
                    status = (
                        exited.si_status
                        if exited.si_code == os.CLD_EXITED
                        else 128 + exited.si_status
                    )
                    return status, bytes(output)
                if time.monotonic() >= deadline:
                    raise RuntimeError("session command timed out")
                if not selector.get_map():
                    time.sleep(0.02)
                for key, _ in selector.select(0.02) if selector.get_map() else []:
                    data = os.read(key.fd, 65536)
                    if not data:
                        selector.unregister(key.fileobj)
                        continue
                    sizes[key.data] += len(data)
                    if sizes[key.data] > 65536:
                        raise RuntimeError("startup utility response exceeded limit")
                    if key.data == "stdout":
                        output.extend(data)
        finally:
            try:
                self.cleanup(child)
            finally:
                selector.close()
                for pipe in (child.stdout, child.stderr):
                    if pipe is not None:
                        pipe.close()

    def capture(self, argv, cwd, timeout=15):
        """Read at most 64 KiB per stream without spooling native diagnostics."""
        status, data = self.execute(
            argv, cwd, timeout, stdout=subprocess.PIPE, stderr=subprocess.PIPE
        )
        if status:
            raise RuntimeError("startup utility failed")
        return data.decode("utf-8")
