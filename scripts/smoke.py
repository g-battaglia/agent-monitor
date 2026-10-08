#!/usr/bin/env python3
"""Synthetic CLI + real PTY lifecycle; no ANSI scraping or provider launches."""
import datetime
import fcntl
import json
import os
import pty
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path

binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/agent-monitor').resolve()
with tempfile.TemporaryDirectory(prefix='am-sessions-') as tmp:
    root = Path(tmp)
    source = root / 'pi-sessions'
    source.mkdir()
    state = root / 'state'
    env = dict(os.environ, PI_CODING_AGENT_SESSION_DIR=str(source),
               PI_CODING_AGENT_DIR=str(root / 'pi'), TMUX_TMPDIR=str(root / 'no-tmux'),
               CLAUDE_CONFIG_DIR=str(root / 'claude'), CODEX_HOME=str(root / 'codex'),
               XDG_DATA_HOME=str(root / 'data'), TERM='xterm-256color', NO_COLOR='1')
    for key in ('TMUX', 'TMUX_PANE', 'AGENT_MONITOR_HOME'):
        env.pop(key, None)

    def cli(*args):
        return subprocess.run([str(binary), '--data-dir', str(state), *args],
                              env=env, capture_output=True, text=True, check=True, timeout=15)

    def session(path, identity, name, timestamp):
        entries = [dict(type='session', version=3, id=identity, cwd=str(root), timestamp=timestamp),
                   dict(type='session_info', id='name', parentId=None, name=name),
                   dict(type='message', id='user', parentId='name', timestamp=timestamp,
                        message=dict(role='user', content='Check the synthetic session.')),
                   dict(type='message', id='reply', parentId='user', timestamp=timestamp,
                        message=dict(role='assistant', content=[dict(type='text', text='Synthetic reply.')]))]
        path.write_text(''.join(json.dumps(e) + '\n' for e in entries))

    session(source / 'old.jsonl', 'old-native', 'Historic onboarding', '2026-01-01T00:00:00Z')
    rows = json.loads(cli('sessions', '--all', '--json').stdout)
    assert len(rows) == 1 and rows[0]['state'] == 'history'
    assert json.loads(cli('sessions', '--json').stdout) == []
    # Created after the persisted baseline; importing a copied old file isn't enough.
    future = (datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(seconds=1)).isoformat()
    session(source / 'new.jsonl', 'new-native', 'New session', future)
    rows = json.loads(cli('sessions', '--json').stdout)
    assert len(rows) == 1 and rows[0]['state'] == 'resume'
    identity = rows[0]['id']
    cli('note', identity, 'Persistent next step')
    cli('done', identity)
    assert json.loads(cli('sessions', '--json').stdout) == []
    cli('undo', identity)
    assert json.loads(cli('sessions', '--json').stdout)[0]['note'] == 'Persistent next step'
    # Launching a provider requires explicit consent; no consent, no spawn.
    denied = subprocess.run([str(binary), '--data-dir', str(state), 'open', identity],
                            env=env, capture_output=True, text=True, timeout=15)
    assert denied.returncode != 0
    # Offline screen classification: pure, no tmux, no agent process.
    screen = root / 'screen.txt'
    screen.write_text('⠼ Working on it\n')
    out = json.loads(cli('agent', 'explain', '--file', str(screen), '--agent', 'codex', '--json').stdout)
    assert out['state'] == 'working' and out['rule'] == 'spinner' and out['manifest'] == 'bundled'
    bad = subprocess.run([str(binary), '--data-dir', str(state), 'agent', 'explain',
                          '--file', str(screen), '--agent', 'bogus'],
                         env=env, capture_output=True, text=True, timeout=15)
    assert bad.returncode != 0

    # All providers use local fixtures; no real home-directory history is read.
    claude = root / 'claude' / 'projects' / 'fixture'
    claude.mkdir(parents=True)
    (claude / 'claude.jsonl').write_text(''.join(json.dumps(e) + '\n' for e in [
        dict(type='mode', sessionId='claude-native'),
        dict(type='user', sessionId='claude-native', cwd=str(root), uuid='cu',
             parentUuid=None, timestamp='2026-01-01T00:00:00Z',
             message=dict(role='user', content='Claude fixture')),
        dict(type='assistant', sessionId='claude-native', uuid='ca', parentUuid='cu',
             message=dict(role='assistant', content=[dict(type='text', text='Claude reply')]))]))
    codex = root / 'codex' / 'sessions'
    codex.mkdir(parents=True)
    (codex / 'codex.jsonl').write_text(''.join(json.dumps(e) + '\n' for e in [
        dict(type='session_meta', payload=dict(id='codex-native', cwd=str(root), timestamp='2026-01-01T00:00:00Z')),
        dict(type='response_item', payload=dict(type='message', role='user', content=[dict(type='input_text', text='Codex fixture')]))]))
    import sqlite3
    opencode = root / 'data' / 'opencode'
    opencode.mkdir(parents=True)
    with sqlite3.connect(opencode / 'opencode.db') as db:
        db.executescript('CREATE TABLE session_v2(id TEXT,directory TEXT,title TEXT,parent_id TEXT,time_created INTEGER,time_updated INTEGER); CREATE TABLE session_message(id TEXT,session_id TEXT,type TEXT,seq INTEGER,time_created INTEGER,data TEXT);')
        db.execute('INSERT INTO session_v2 VALUES(?,?,?,?,?,?)', ('ses_fixture', str(root), 'OpenCode fixture', None, 1, 2))
        db.execute('INSERT INTO session_message VALUES(?,?,?,?,?,?)', ('om', 'ses_fixture', 'user', 1, 1, json.dumps(dict(text='OpenCode request'))))
    rows = json.loads(cli('sessions', '--all', '--json').stdout)
    assert {s['metadata']['provider'] for s in rows} == {'pi', 'claude', 'codex', 'opencode'}
    for provider in ('claude', 'codex', 'opencode'):
        saved = next(s for s in rows if s['metadata']['provider'] == provider)
        preview = json.loads(cli('show', saved['id'], '--json').stdout)
        assert preview['conversation']['messages'] and not preview['conversation']['partial']
    opened = json.loads(cli('sessions', '--open', '--json').stdout)
    assert 'sessions' in opened and 'panes' in opened

    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 32, 120, 0, 0))

    def controlling_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    process = subprocess.Popen([str(binary), '--data-dir', str(state)],
                               stdin=slave, stdout=slave, stderr=slave, env=env,
                               preexec_fn=controlling_terminal)
    os.close(slave)

    def drain(seconds):
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            ready, _, _ = select.select([master], [], [], max(0, until - time.monotonic()))
            if ready:
                try:
                    if not os.read(master, 65536):
                        break
                except OSError:
                    break

    try:
        drain(1.2)
        assert process.poll() is None
        os.write(master, b'nNote from the TUI\r')
        drain(0.8)
        os.write(master, b'd')
        drain(0.8)
        row = next(s for s in json.loads(cli('sessions', '--all', '--json').stdout) if s['id'] == identity)
        assert row['state'] == 'done'
        assert row['note'].endswith('Note from the TUI')
        # Exercise project/filter/detail panels and a small terminal resize.
        os.write(master, b'fjj\r\tzk\x1b')
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 12, 40, 0, 0))
        drain(0.4)
        start = time.monotonic()
        os.write(master, b'q')
        drain(0.2)
        process.wait(timeout=3)
        assert process.returncode == 0
        assert time.monotonic() - start < 1
        print('PASS: four-provider sessions, previews, resume states, notes, undo, PTY resize and quit')
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=3)
        os.close(master)
