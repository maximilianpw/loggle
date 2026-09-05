"""Linux PTY integration against the real runtime-bundled executable. Stdlib only."""
import fcntl
import os
from pathlib import Path
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time

EXE = str(Path(sys.argv[1]).resolve())


class Terminal:
    def __init__(self, commands):
        self.master, self.slave = os.openpty()
        self.saved = termios.tcgetattr(self.slave)
        self.resize(26, 110)

        def child():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        self.proc = subprocess.Popen([EXE] + commands, stdin=self.slave,
                                     stdout=self.slave, stderr=self.slave, preexec_fn=child)
        self.data = b""

    def resize(self, rows, cols):
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

    def read(self, seconds=0.15):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            if select.select([self.master], [], [], min(0.03, max(0, end-time.monotonic())))[0]:
                try:
                    chunk = os.read(self.master, 65536)
                except OSError:
                    break
                self.data = (self.data + chunk)[-512_000:]
        return self.data

    def expect(self, text, timeout=5):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if text in self.read():
                return
        raise AssertionError(f"missing {text!r}: {self.data[-5000:]!r}")

    def key(self, keys):
        self.data = b""
        os.write(self.master, keys)

    def quit(self, keys=b"q"):
        start = time.monotonic()
        self.key(keys)
        while self.proc.poll() is None and time.monotonic()-start < 3:
            self.read(0.05)
        assert self.proc.poll() == 0, self.data[-2000:]
        elapsed = time.monotonic()-start
        assert elapsed < 2, elapsed
        assert termios.tcgetattr(self.slave) == self.saved, "terminal settings not restored"
        print(f"quit + terminal restoration: {elapsed:.3f}s")

    def close(self):
        if self.proc.poll() is None:
            self.key(b"\x03")
            try:
                self.proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        os.close(self.master)
        os.close(self.slave)


def gone(pid):
    try:
        # Container init may not reap orphan zombies; zombies cannot execute/hold pipes.
        return Path(f"/proc/{pid}/stat").read_text().split(") ")[1][0] == "Z"
    except FileNotFoundError:
        return True


def wait_gone(pids):
    end = time.monotonic() + 3
    while time.monotonic() < end and not all(gone(p) for p in pids):
        time.sleep(0.03)
    assert all(gone(p) for p in pids), f"live descendants: {[p for p in pids if not gone(p)]}"


def group_members(leaders):
    members = []
    for path in Path("/proc").glob("[0-9]*/stat"):
        try:
            fields = path.read_text().split(") ")[1].split()
            if int(fields[2]) in leaders:
                members.append(int(path.parent.name))
        except FileNotFoundError:
            pass
    return members


def smoke():
    t = Terminal(["api=printf 'INFO ready\\n'; printf 'ERROR bad\\n' >&2; exit 7",
                  "web=printf '%s\\n' '{\"level\":\"warn\",\"message\":\"retry job\"}'; printf partial"])
    try:
        for text in [b"api: exit 7", b"web: exit 0", b"out warn", b"err error", b"retry job", b"partial"]:
            t.expect(text)
        t.key(b"p")
        t.expect(b"PAUSED")
        t.key(b"/retry\r")
        t.expect(b"text=retry")
        t.read(0.3)
        frame = t.data.split(b"\x1b[H")[-1]
        assert b"retry job" in frame and b"INFO ready" not in frame
        t.key(b"cGss")
        t.expect(b"source=web")
        t.key(b"/no-match\r")
        t.expect(b"text=no-match")
        t.resize(8, 32)
        t.read(0.2)
        t.resize(26, 110)
        t.key(b"cG")
        t.expect(b"retry job")
        t.quit()
        print("PASS: two commands, stdout/stderr, JSON/plain, statuses, pause/filter/source/resize")
    finally:
        t.close()


def flood(abrupt=False):
    with tempfile.TemporaryDirectory(prefix="loggle-test-") as tmp:
        ids = Path(tmp) / "pids"
        # Both shell and grandchildren ignore graceful signals, and fill both pipes.
        cmd = f"trap '' INT TERM; echo $$ >> {ids}; (trap '' INT TERM; yes 'INFO flood') & (trap '' INT TERM; yes 'ERROR flood' >&2) & wait"
        web = f"echo $$ >> {ids}; yes '{{\"level\":\"warn\",\"message\":\"flood\"}}'"
        t = Terminal(["api=" + cmd, "web=" + web])
        try:
            t.expect(b"retained 2000")
            leaders = [int(p) for p in ids.read_text().split()]
            assert len(leaders) == 2
            pids = group_members(leaders)
            assert len(pids) >= 5, pids
            rss = []
            for _ in range(40):
                t.read(0.3)
                status = Path(f"/proc/{t.proc.pid}/status").read_text()
                rss.append(int(status.split("VmRSS:")[1].split()[0]))
            assert max(rss)-min(rss) < 32*1024, rss
            assert max(rss) < 256*1024, rss
            print(f"flood RSS KiB: min={min(rss)} max={max(rss)}")
            t.key(b"p/no-match\r")
            t.expect(b"text=no-match")
            if abrupt:
                t.proc.kill()  # Deliberate VM SIGKILL tests native EOF cleanup.
                t.proc.wait(timeout=2)
                wait_gone(pids)
                time.sleep(0.2)
                assert termios.tcgetattr(t.slave) == t.saved
            else:
                t.quit(b"\x03")
                wait_gone(pids)
            print(f"PASS: flood bounds, responsive input, {len(pids)} group members cleaned (VM kill={abrupt})")
        finally:
            t.close()


def normal_exit_descendants():
    with tempfile.TemporaryDirectory(prefix="loggle-test-") as tmp:
        pidfile = Path(tmp) / "pid"
        t = Terminal([f"api=(trap '' INT TERM; sleep 1000) & echo $! > {pidfile}; exit 3"])
        try:
            t.expect(b"api: exit 3")
            wait_gone([int(pidfile.read_text())])
            t.quit()
            print("PASS: shell exit cleans surviving background descendant")
        finally:
            t.close()


def extremes_and_errors():
    t = Terminal(["tiny=yes ''", "long=cat /dev/zero"])
    try:
        t.expect(b"retained 2000")
        t.read(1)
        t.key(b"/nothing\r")
        t.expect(b"text=nothing", timeout=2)
        t.quit()
        print("PASS: worst newline batch + unterminated flood remain interactive")
    finally:
        t.close()
    t = Terminal(["missing=loggle-intentionally-missing-command", "stdin=read answer"])
    try:
        t.expect(b"missing: exit 127")
        t.expect(b"stdin: exit 1")
        t.quit()
        print("PASS: launch errors and closed child stdin")
    finally:
        t.close()


smoke()
flood()
flood(abrupt=True)
normal_exit_descendants()
extremes_and_errors()
print("ALL PTY CHECKS PASSED")
