"""Generate a tiny original GBA button task; no commercial ROM/BIOS required."""
import argparse
import json
from pathlib import Path
import struct

def generate(directory):
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    rom = bytearray(192)
    struct.pack_into('<I', rom, 0, 0xea00002e)  # Branch over the cartridge header.
    rom[0xa0:0xac] = b'TINYBIRD AI '
    rom[0xac:0xb0] = b'TBAI'
    # Poll KEYINPUT. Write 0 for RIGHT held, otherwise 1, into EWRAM.
    words = [0xe59f0014, 0xe59f1014, 0xe1d120b0, 0xe2022010,
             0xe1a02222, 0xe5802000, 0xeafffffa, 0x02000000, 0x04000130]
    rom.extend(struct.pack('<' + 'I' * len(words), *words))
    observations = {'schema_version': 1, 'game': {'code': 'TBAI', 'revision': 0},
                    'fields': [{'name': 'task.distance', 'address': 0x02000000, 'width': 4}]}
    task = {'schema_version': 1, 'frame_skip': 1, 'max_steps': 4, 'history': 1,
            'actions': [[], ['LEFT'], ['RIGHT']],
            'features': [{'field': 'task.distance', 'min': 0, 'max': 1}],
            'step_reward': -.01,
            'rewards': [{'field': 'task.distance', 'mode': 'progress', 'scale': -1}],
            'terminal': [{'field': 'task.distance', 'op': 'eq', 'value': 0, 'outcome': 'success', 'reward': 1}]}
    (directory / 'buttons.gba').write_bytes(rom)
    for name, value in [('observations.json', observations), ('task.json', task)]:
        (directory / name).write_text(json.dumps(value, indent=2))
    return directory / 'buttons.gba', observations, task

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=Path('target/training-demo'))
    args = parser.parse_args()
    generate(args.output)
    print(f'Generated demo in {args.output}')
