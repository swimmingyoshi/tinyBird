"""Check candidate Git files without printing secret values. Not a full secret audit."""
import re
import subprocess
from pathlib import Path

paths = subprocess.check_output(['git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z']).decode().split('\0')
secrets = []
for path in [Path('.env'), Path('.env.development'), Path('deploy/tinybird.env')]:
    if path.is_file():
        for line in path.read_text().splitlines():
            key, separator, value = line.partition('=')
            value = value.strip().strip('\"\'')
            if separator and re.search('KEY|SECRET|TOKEN|PASSWORD', key) and len(value) >= 12:
                secrets.append(value.encode())
findings = []
for name in filter(None, paths):
    path = Path(name)
    if not path.is_file(): continue
    contents = path.read_bytes()
    if any(secret in contents for secret in secrets): findings.append(name)
    if path.suffix.lower() in ['.gba', '.bin', '.state', '.sav', '.sqlite3', '.sqlite', '.db', '.pem', '.key'] or path.name in ['.env', '.env.development', 'tinybird.env']:
        findings.append(name)
    if b'-----BEGIN PRIVATE KEY-----' in contents or b'-----BEGIN RSA PRIVATE KEY-----' in contents:
        # This checker contains the marker literals but no actual key block.
        if path.name != 'check-release.py': findings.append(name)
if findings:
    print('Review before staging/pushing (values withheld):\n' + '\n'.join(sorted(set(findings))))
    raise SystemExit(1)
print('PASS: no forbidden payload files or configured-secret matches in Git candidates.')
