#!/usr/bin/env python3
"""10k-file catalog + PTY responsiveness, using disposable sessions only."""
import datetime
import fcntl
import json
import os
import pty
import resource
import select
import sqlite3
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/release/agent-monitor').resolve()
with tempfile.TemporaryDirectory(prefix='am-latency-') as tmp:
    root = Path(tmp)
    source = root / 'sessions'
    source.mkdir()
    state = root / 'state'
    stamp = (datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(days=1)).isoformat()
    for i in range(10_000):
        header = dict(type='session', version=3, id=f'native-{i}', cwd=str(root / f'project-{i % 100}'), timestamp=stamp)
        name = dict(type='session_info', id='name', parentId=None, name=f'Session {i:05d}')
        message = dict(type='message', id='user', parentId='name', timestamp=stamp,
                       message=dict(role='user', content='Synthetic conversation.'))
        (source / f'{i}.jsonl').write_text('\n'.join(map(json.dumps, (header, name, message))) + '\n')
    # Include a branching-reader workload larger than one conversation page.
    large_stamp=(datetime.datetime.now(datetime.timezone.utc)+datetime.timedelta(days=2)).isoformat()
    header = dict(type='session',version=3,id='large-native',cwd=str(root/'project-0'),timestamp=large_stamp)
    with (source/'large.jsonl').open('w') as file:
        file.write(json.dumps(header)+'\n')
        file.write(json.dumps(dict(type='session_info',id='name',parentId=None,name='Large session'))+'\n')
        for i in range(20_000):
            entry=dict(type='message',id=f'entry-{i}',parentId='name' if i==0 else f'entry-{i-1}',timestamp=large_stamp,
                       message=dict(role='user',content='Synthetic text. '*60))
            file.write(json.dumps(entry)+'\n')
    env = dict(os.environ, PI_CODING_AGENT_SESSION_DIR=str(source),
               PI_CODING_AGENT_DIR=str(root / 'pi'), TMUX_TMPDIR=str(root / 'no-tmux'),
               TERM='xterm-256color', NO_COLOR='1')
    for key in ('TMUX', 'TMUX_PANE', 'AGENT_MONITOR_HOME'):
        env.pop(key, None)
    start = time.monotonic()
    subprocess.run([str(binary), '--data-dir', str(state), 'sync'], env=env, capture_output=True, check=True, timeout=120)
    print(f'Initial catalog (10k JSONL + 20k-entry transcript): {time.monotonic() - start:.2f} s')
    connection = sqlite3.connect(state / 'sessions.db')
    assert connection.execute('select count(*) from sessions').fetchone()[0] == 10_001
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 32, 120, 0, 0))

    def terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    start = time.monotonic()
    process = subprocess.Popen([str(binary), '--data-dir', str(state)], stdin=slave, stdout=slave,
                               stderr=slave, env=env, preexec_fn=terminal)
    os.close(slave)

    def drain(seconds):
        until = time.monotonic() + seconds
        data = b''
        while time.monotonic() < until:
            ready, _, _ = select.select([master], [], [], max(0, until - time.monotonic()))
            if ready:
                try:
                    part = os.read(master, 65536)
                    if not part:
                        break
                    data += part
                except OSError:
                    break
        return data

    try:
        # A named row confirms the cached catalog was drawn, not just raw-mode entry.
        until = start + 3
        output = b''
        while b'Session' not in output and time.monotonic() < until:
            output += drain(0.02)
        cached = (time.monotonic() - start) * 1000
        assert b'Session' in output
        print(f'First cached named row: {cached:.1f} ms')
        assert cached < 500
        drain(0.4)
        times = []
        for _ in range(40):
            drain(0.03)
            sent = time.monotonic()
            os.write(master, b'j')
            ready, _, _ = select.select([master], [], [], 0.5)
            assert ready
            os.read(master, 65536)
            times.append((time.monotonic() - sent) * 1000)
        times.sort()
        print(f'PTY key-to-output: p95 {times[38]:.1f} ms (buffer rendering verified separately)')
        assert times[38] < 50
        # Make a mutation wait on SQLite. Input and quit must stay independent.
        connection.execute('begin immediate')
        os.write(master, b'npending\r')
        drain(0.8)  # Let the entire note editor keystroke sequence be processed.
        sent = time.monotonic()
        os.write(master, b'q')
        while process.poll() is None and time.monotonic() - sent < 2:
            drain(0.005)  # Keep the PTY writer unblocked during terminal restore.
        process.wait(timeout=1)
        quitting = (time.monotonic() - sent) * 1000
        print(f'Quit with SQLite writer blocked: {quitting:.1f} ms')
        assert process.returncode == 0 and quitting < 200
    finally:
        connection.rollback()
        connection.close()
        if process.poll() is None:
            process.kill()
            drain(0.05)
            process.wait(timeout=5)
        os.close(master)
        rss=resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
        print(f'Catalog/PTY peak RSS: {rss/(1024*1024 if sys.platform=="darwin" else 1024):.1f} MiB')
