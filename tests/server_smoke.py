"""Start the real local server and check what it accepts and serves.

No remote services and no personal data: a temporary working directory, no
ROMs, no BIOS. Run with the path to a built `tinybird-web` binary.
"""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument('executable', type=Path)
args = parser.parse_args()
executable = args.executable.resolve()


def call(port, path, method='GET', origin=None, host=None, headers=None):
    sent = dict(headers or {})
    if origin: sent['Origin'] = origin
    if host: sent['Host'] = host
    request = urllib.request.Request(f'http://127.0.0.1:{port}{path}', method=method, headers=sent)
    try:
        with urllib.request.urlopen(request, timeout=3) as response:
            return response.status, response.read(), response.headers
    except urllib.error.HTTPError as error:
        return error.code, error.read(), error.headers


def clean_env():
    return {k: v for k, v in os.environ.items() if not k.startswith('TINYBIRD_')}


with tempfile.TemporaryDirectory() as directory:
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0)); port = listener.getsockname()[1]
    process = subprocess.Popen([str(executable), '--port', str(port)], cwd=directory, env=clean_env(), stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        for attempt in range(100):
            if process.poll() is not None: raise AssertionError('Server failed to start: ' + process.stderr.read().decode())
            try:
                if call(port, '/api/health')[0] == 200: break
            except OSError: pass
            time.sleep(.05)
        else: raise AssertionError('Server did not become ready')

        health = json.loads(call(port, '/api/health')[1])
        assert health['ok'] is True and health['bios_ready'] is False and health['roms'] == 0, health
        origin = f'http://127.0.0.1:{port}'

        # Writes and WebSocket handshakes must come from this server's own pages.
        assert call(port, '/api/health', 'POST')[0] == 403
        assert call(port, '/api/health', 'POST', 'https://evil.example')[0] == 403
        assert call(port, '/api/health', 'POST', origin)[0] != 403
        assert call(port, '/api/health', headers={'Upgrade': 'websocket'})[0] == 403
        # A request naming another host is DNS rebinding, whatever it asks for.
        assert call(port, '/api/health', host='rebind.example')[0] == 403

        for path in ['/', '/play', '/addons', '/reader-guide']:
            status, _, headers = call(port, path)
            assert status == 200, path
            assert headers['X-Content-Type-Options'] == 'nosniff', path
        assert json.loads(call(port, '/api/local')[1]) == {'assets': []}
        assert call(port, '/bios')[0] == 404

        # Static assets revalidate instead of downloading again.
        status, _, headers = call(port, '/play.js')
        assert status == 200 and headers['Cache-Control'] == 'no-cache' and headers['ETag']
        assert call(port, '/play.js', headers={'If-None-Match': headers['ETag']})[0] == 304
        assert call(port, '/api/health')[2]['Cache-Control'] == 'no-store'

        # The hosted site is not part of this server.
        for path in ['/api/auth/me', '/api/saves', '/api/library', '/api/lobby', '/api/shots',
                     '/api/community-addons', '/api/deployment', '/contact', '/support/tickets', '/info']:
            assert call(port, path)[0] == 404, path
        print('PASS: local startup, request origins, assets and route boundaries')
    finally:
        process.terminate(); process.wait(timeout=10); process.stderr.close()

with tempfile.TemporaryDirectory() as directory:
    for extra in [['--host', '0.0.0.0'], ['--host', '192.168.1.10']]:
        result = subprocess.run([str(executable), *extra], cwd=directory, env=clean_env(), capture_output=True, timeout=10)
        assert result.returncode != 0, extra
print('PASS: binding beyond this machine fails startup')
