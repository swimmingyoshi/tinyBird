"""Exercise real server mode boundaries without remote services or personal data."""
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

def call(port, path, method='GET', origin=None, host=None):
    headers = {}
    if origin: headers['Origin'] = origin
    if host: headers['Host'] = host
    request = urllib.request.Request(f'http://127.0.0.1:{port}{path}', method=method, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=3) as response:
            return response.status, response.read(), response.headers
    except urllib.error.HTTPError as error:
        return error.code, error.read(), error.headers

for mode in ['local', 'production', 'development']:
    with tempfile.TemporaryDirectory() as directory:
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0)); port = listener.getsockname()[1]
        env = {k: v for k, v in os.environ.items() if not k.startswith('TINYBIRD_')}
        if mode == 'production': env['TINYBIRD_PUBLIC_ORIGIN'] = 'https://tinybird.example'
        if mode == 'local':
            env.update(TINYBIRD_AUTH_PROJECT_SECRET='unused-test-secret', TINYBIRD_MEDIA_KEY='unused-test-key', TINYBIRD_CONTACT_KEY='unused-test-key', TINYBIRD_ADDON_DB=str(Path(directory) / 'must-not-exist.sqlite3'))
        process = subprocess.Popen([str(executable), '--mode', mode, '--port', str(port)], cwd=directory, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            for attempt in range(100):
                if process.poll() is not None: raise AssertionError('Server failed to start: ' + process.stderr.read().decode())
                try:
                    if call(port, '/api/health')[0] == 200: break
                except OSError: pass
                time.sleep(.05)
            else: raise AssertionError('Server did not become ready')
            assert json.loads(call(port, '/api/deployment')[1])['mode'] == mode
            origin = 'https://tinybird.example' if mode == 'production' else f'http://127.0.0.1:{port}'
            assert call(port, '/api/auth/logout', 'POST')[0] == 403
            assert call(port, '/api/auth/logout', 'POST', 'https://evil.example')[0] == 403
            assert call(port, '/api/auth/logout', 'POST', origin)[0] != 403
            assert call(port, '/api/lobby/ws', origin='https://evil.example')[0] == 403
            if mode != 'production': assert call(port, '/api/health', host='rebind.example')[0] == 403
            if mode == 'local':
                assert json.loads(call(port, '/api/auth/me')[1])['configured'] is False
                assert not (Path(directory) / 'must-not-exist.sqlite3').exists()
                assert call(port, '/api/lobby', 'POST', origin)[0] == 404
                assert call(port, '/api/community-addons', 'POST', origin)[0] != 200
                assert call(port, '/contact')[0] == 404
            if mode == 'production':
                status, body, headers = call(port, '/api/health')
                assert json.loads(body) == {'ok': True}
                assert headers['Strict-Transport-Security'] == 'max-age=31536000'
                for path in ['/bios', '/api/snapshot', '/overlay', '/local/game.gba']:
                    assert call(port, path)[0] == 404, path
                assert call(port, '/api/library/upload', 'POST', origin)[0] == 404
                assert json.loads(call(port, '/api/local')[1]) == {'assets': []}
            assert call(port, '/play')[2]['X-Content-Type-Options'] == 'nosniff'
            print(f'PASS: {mode} startup, request origins and route boundaries')
        finally:
            process.terminate(); process.wait(timeout=10); process.stderr.close()

with tempfile.TemporaryDirectory() as directory:
    for extra in [['--mode', 'production'], ['--mode', 'local', '--host', '0.0.0.0'], ['--mode', 'typo']]:
        env = {k: v for k, v in os.environ.items() if not k.startswith('TINYBIRD_')}
        result = subprocess.run([str(executable), *extra], cwd=directory, env=env, capture_output=True, timeout=10)
        assert result.returncode != 0
print('PASS: unsafe and unknown configurations fail startup')
