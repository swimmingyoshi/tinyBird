"""Build an allowlisted local ZIP. Never copy a working tree or environment file."""
import argparse
import hashlib
from pathlib import Path
import platform
import zipfile

parser = argparse.ArgumentParser()
parser.add_argument('--bin-dir', type=Path, default=Path('target/release'))
parser.add_argument('--wasm', type=Path, default=Path('target/wasm32-unknown-unknown/release/tinybird_wasm.wasm'))
parser.add_argument('--output', type=Path, default=Path('target/dist'))
args = parser.parse_args()
suffix = '.exe' if platform.system() == 'Windows' else ''
files = {f'tinybird-web{suffix}': args.bin_dir / f'tinybird-web{suffix}',
         f'tinybird-headless{suffix}': args.bin_dir / f'tinybird-headless{suffix}',
         'tinybird_wasm.wasm': args.wasm, 'LICENSE': Path('LICENSE'),
         'README.md': Path('docs/local-edition.md'),
         'automation.md': Path('docs/automation.md'),
         'session-recovery.md': Path('docs/session-recovery.md')}
for name in ['tinybird.py', 'run_observations.py']:
    files[f'python/{name}'] = Path('examples/python') / name
for path in Path('addons').glob('*.json'):
    files[f'addons/{path.name}'] = path
for path in files.values():
    if not path.is_file():
        raise SystemExit(f'Missing required artifact: {path}. Build the web, runtime and WASM packages first.')
args.output.mkdir(parents=True, exist_ok=True)
destination = args.output / f'tinybird-local-{platform.system().lower()}-{platform.machine().lower()}.zip'
launcher = '@echo off\r\ncd /d "%~dp0"\r\ntinybird-web.exe --mode local --host 127.0.0.1 --port 8877 --wasm tinybird_wasm.wasm --roms roms --bios gba_bios.bin\r\npause\r\n' if suffix else '#!/bin/sh\ncd -- "$(dirname -- "$0")" || exit 1\nexec ./tinybird-web --mode local --host 127.0.0.1 --port 8877 --wasm tinybird_wasm.wasm --roms roms --bios gba_bios.bin\n'
with zipfile.ZipFile(destination, 'w', zipfile.ZIP_DEFLATED) as archive:
    for name, path in files.items():
        archive.write(path, name)
    info = zipfile.ZipInfo('start-local.cmd' if suffix else 'start-local.sh')
    info.external_attr = 0o100755 << 16
    archive.writestr(info, launcher)
digest = hashlib.sha256(destination.read_bytes()).hexdigest()
destination.with_suffix('.zip.sha256').write_text(f'{digest}  {destination.name}\n')
print(f'Created {destination} ({len(files) + 1} allowlisted files)')
