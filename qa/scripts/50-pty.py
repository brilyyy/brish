#!/usr/bin/env python3
"""Drive a shell over a real pty and report what it did.

brish's interactive path is gated on stdin AND stdout both being TTYs
(crates/brish/src/main.rs, `edit_repl`). That makes completion,
highlighting, autosuggest, menus, keymaps, history search and the
transient prompt unreachable by any non-pty test. This driver is the
only way to exercise them.

Why not `script -qc`: it allocates a pty but gives no way to wait for a
prompt between keystrokes, so every assertion becomes a timing guess.
Here we can type, wait for a sentinel to appear, and keep going.

Usage:
   pty.py --shell 'brish -i' [--cwd DIR] [--rows N] [--cols N]
         [--send TEXT]...        type TEXT (+Enter by default)
         [--raw TEXT]...         type TEXT verbatim, no Enter
         [--settle SECONDS]      wait after each send (default 0.35)
         [--expect TEXT]         assert TEXT appears; exit 1 if not
         [--measure TEXT]        also report seconds until TEXT appears
         [--timeout SECONDS]     overall budget (default 10)
         [--env K=V]...          extra env for the child
         [--bg-pgrp]             run the shell in a process group that is
                                  NOT the terminal's foreground group

Exit codes: 0 ok, 1 expectation not met / child failed, 124 timeout.
Transcript (CR/LF normalised) goes to stdout; measurement to stderr.
"""

import argparse
import os
import pty
import select
import shlex
import signal
import sys
import time

# Window geometry, used both to size the pty and to synthesise the
# cursor-position reply a real terminal would send. Set from --rows/--cols
# in main() before any child output is read.
ROWS = 24
COLS = 80


def build_parser():
    p = argparse.ArgumentParser(add_help=True)
    p.add_argument("--shell", required=True, help="command line to run under the pty")
    p.add_argument("--cwd", default=None)
    p.add_argument("--rows", type=int, default=24)
    p.add_argument("--cols", type=int, default=80)
    p.add_argument("--send", action="append", default=[])
    p.add_argument("--raw", action="append", default=[])
    p.add_argument("--settle", type=float, default=0.35)
    p.add_argument("--expect", default=None)
    p.add_argument("--measure", default=None)
    p.add_argument("--first-output", action="store_true",
                   help="measure seconds until the first output byte")
    p.add_argument("--timeout", type=float, default=10.0)
    p.add_argument("--env", action="append", default=[])
    p.add_argument("--bg-pgrp", action="store_true",
                   help="start the shell in a background process group "
                        "(how login(1)/Terminal.app launches a login shell)")
    p.add_argument("--teardown", default="\x03exit\n",
                   help="keys sent before killing; empty to skip")
    return p


def drain(fd, seconds, sink):
    """Read from fd for up to `seconds`, appending to sink.

    Also answers the terminal capability probes a real TTY would answer.
    crossterm (via reedline) queries cursor position with ESC[6n on
    startup and BLOCKS until the reply arrives; a bare pty slave answers
    nothing, so the shell hangs and then exits 0 with no output. Feeding
    back a plausible DSR reply is what makes a raw pty usable as a
    terminal stand-in.
    """
    end = time.monotonic() + seconds
    got = b""
    while True:
        left = end - time.monotonic()
        if left <= 0:
            break
        try:
            r, _, _ = select.select([fd], [], [], left)
        except (OSError, ValueError):
            break
        if not r:
            continue
        try:
            chunk = os.read(fd, 65536)
        except OSError:
            break
        if not chunk:
            break
        got += chunk
        sink.append(chunk)
        reply = terminal_replies(got)
        if reply:
            try:
                os.write(fd, reply)
            except OSError:
                pass
    return got


def terminal_replies(seen):
    """Replies owed to the child for capability queries in `seen`.

    ESC[6n -> DSR cursor position: answer ESC[<row>;<col>R. We report
    the middle of the window; nothing here asserts on exact columns.
    """
    reply = b""
    if b"\x1b[6n" in seen:
        reply += b"\x1b[%d;%dR" % (ROWS // 2, COLS // 2)
    return reply


def exec_shell(args, env):
    """Replace this process with the shell under test."""
    if args.cwd:
        os.chdir(args.cwd)
    os.environ.clear()
    os.environ.update(env)
    if args.bg_pgrp:
        # Become a fresh process group, leaving the terminal's foreground
        # group with the session leader. Done here, before exec, so there
        # is no race with the parent's setpgid — if the shell execs first
        # it is still in the foreground group and the shape is not tested.
        os.setpgid(0, 0)
        # POSIX: exec preserves ignored signals, and /bin/sh ignores the
        # job-control trio for its own job control. Without this reset the
        # shell under test inherits SIGTTOU=SIG_IGN and cannot possibly
        # reproduce the bug. login(1) (the real launcher) starts the shell
        # with default dispositions, so default is the faithful state.
        for s in (signal.SIGTTOU, signal.SIGTTIN, signal.SIGTSTP):
            signal.signal(s, signal.SIG_DFL)
        # ...and exec the shell DIRECTLY. An intermediate /bin/sh ignores
        # the job-control trio for its own sake, so it would hand the shell
        # under test SIGTTOU=SIG_IGN and mask the bug a second time.
        os.execvp(args.shell.split()[0], shlex.split(args.shell))
    os.execvp("/bin/sh", ["/bin/sh", "-c", args.shell])


def main():
    global ROWS, COLS
    args = build_parser().parse_args()
    ROWS, COLS = args.rows, args.cols

    env = dict(os.environ)
    for kv in args.env:
        if "=" in kv:
            k, v = kv.split("=", 1)
            env[k] = v

    # `--bg-pgrp` puts the shell in a process group that is NOT the
    # terminal's foreground group. That is how Terminal.app starts a login
    # shell (via login(1)), and it is the one startup shape a plain
    # pty.fork() never produces: the fork child IS the session leader, so
    # its group is already foreground and a shell calling setpgid(0,0) +
    # tcsetpgrp() cannot stop itself. Needs an extra fork (to own a
    # different group) plus a pipe to hand the shell's pid back, since
    # the session leader — not the shell — is what pty.fork() returns.
    pid_r, pid_w = os.pipe()
    pid, fd = pty.fork()
    if pid == 0:
        # child
        try:
            os.close(pid_r)
            if args.bg_pgrp:
                g = os.fork()
                if g == 0:
                    exec_shell(args, env)
                os.setpgid(g, g)      # new group, terminal stays with `pid`
                os.write(pid_w, b"%d" % g)
                os.close(pid_w)
                os.waitpid(g, 0)      # hold the foreground group open
                os._exit(0)
            exec_shell(args, env)
        except Exception as exc:  # pragma: no cover - child bail-out
            sys.stderr.write(str(exc))
            os._exit(127)

    # parent
    os.close(pid_w)
    shell_pid = pid
    if args.bg_pgrp:
        shell_pid = int(os.read(pid_r, 32) or b"0") or pid
        os.close(pid_r)
    import fcntl
    import struct
    import termios

    fcntl.ioctl(fd, termios.TIOCSWINSZ,
                struct.pack("HHHH", args.rows, args.cols, 0, 0))

    sink = []
    first_out_at = None
    t0 = time.monotonic()

    def note_first_output():
        nonlocal first_out_at
        if first_out_at is None and sink:
            first_out_at = time.monotonic() - t0

    drain(fd, args.settle, sink)
    note_first_output()

    for text in args.raw:
        os.write(fd, text.encode())
        drain(fd, args.settle, sink)
        note_first_output()

    for text in args.send:
        os.write(fd, (text + "\n").encode())
        drain(fd, args.settle, sink)
        note_first_output()

    measured = None
    if args.measure:
        want = args.measure.encode()
        start = time.monotonic()
        buf = b"".join(sink)
        deadline = start + args.timeout
        while want not in buf and time.monotonic() < deadline:
            drain(fd, 0.05, sink)
            buf = b"".join(sink)
        if want in buf:
            measured = time.monotonic() - start
        else:
            measured = None

    if args.expect:
        want = args.expect.encode()
        buf = b"".join(sink)
        deadline = time.monotonic() + args.timeout
        while want not in buf and time.monotonic() < deadline:
            drain(fd, 0.05, sink)
            buf = b"".join(sink)
        if want not in buf:
            drain(fd, 0.2, sink)

    if args.teardown:
        try:
            os.write(fd, args.teardown.encode())
        except OSError:
            pass

    # Reap without hanging: a shell that ignored Ctrl-C would otherwise
    # wedge the whole suite. `--bg-pgrp` watches the shell, not the
    # session leader that pty.fork() handed us.
    status = None
    end = time.monotonic() + 2.0
    while time.monotonic() < end:
        try:
            done, st = os.waitpid(shell_pid, os.WNOHANG)
        except ChildProcessError:
            break
        if done == shell_pid:
            status = st
            break
        drain(fd, 0.05, sink)
    if status is None:
        try:
            os.kill(shell_pid, signal.SIGKILL)
            os.waitpid(shell_pid, 0)
        except (ProcessLookupError, ChildProcessError):
            pass
    if shell_pid != pid:
        # The session leader exits once its shell is reaped.
        try:
            os.waitpid(pid, os.WNOHANG)
        except ChildProcessError:
            pass

    transcript = b"".join(sink).replace(b"\r\n", b"\n").replace(b"\r", b"\n")
    sys.stdout.buffer.write(transcript)
    sys.stdout.buffer.flush()

    if measured is not None:
        sys.stderr.write("LATENCY={:.4f}\n".format(measured))

    if args.first_output:
        if first_out_at is None:
            sys.stderr.write("LATENCY=none\n")
        else:
            sys.stderr.write("LATENCY={:.4f}\n".format(first_out_at))

    ok = True
    if args.expect and args.expect.encode() not in transcript:
        ok = False
    if args.measure and measured is None:
        ok = False
    if args.first_output and first_out_at is None:
        ok = False
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()