"""End-to-end protocol/client regression using a synthetic ARM ROM."""
from pathlib import Path
import os
import tempfile
import unittest
import json
import subprocess

from tinybird import TinyBird


class RuntimeTest(unittest.TestCase):
    def test_workshop_configuration_and_cli(self):
        root = Path(__file__).resolve().parents[2]
        binary = root / "target" / "debug" / ("tinybird-headless.exe" if os.name == "nt" else "tinybird-headless")
        config = root / "tests" / "fixtures" / "observations.json"
        with tempfile.TemporaryDirectory() as directory:
            rom = Path(directory) / "loop.gba"
            data = bytearray(bytes.fromhex("feffffea") + bytes(188))
            data[0xac:0xb0] = b"TBST"
            data[0xbc] = 2
            rom.write_bytes(data)
            with TinyBird(rom, executable=binary, observations=config) as env:
                self.assertEqual(env.reset()["memory"], {"player.hp": 0})
                self.assertEqual(env.step(["RIGHT"], frames=2)["memory"], {"player.hp": 0})
                bad = json.loads(config.read_text())
                bad["game"]["revision"] = 3
                with self.assertRaisesRegex(RuntimeError, "does not match"):
                    env.configure_observations(bad)
                self.assertEqual(env.get_state()["memory"], {"player.hp": 0})
            result = subprocess.run([str(binary), str(rom), "--observations", str(config)],
                                    input='{"op":"get_state"}\n', text=True, capture_output=True, check=True)
            self.assertEqual(json.loads(result.stdout)["result"]["memory"], {"player.hp": 0})
            data[0xbc] = 3
            rom.write_bytes(data)
            result = subprocess.run([str(binary), str(rom), "--observations", str(config)],
                                    input='', text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("does not match", result.stderr)

    def test_process_protocol_and_replay(self):
        root = Path(__file__).resolve().parents[2]
        binary = root / "target" / "debug" / (
            "tinybird-headless.exe" if os.name == "nt" else "tinybird-headless"
        )
        with tempfile.TemporaryDirectory() as directory:
            rom = Path(directory) / "loop.gba"
            rom.write_bytes(bytes.fromhex("feffffea") + bytes(188))
            with TinyBird(rom, executable=binary) as env:
                initial = env.reset()
                env.set_observations([
                    dict(name="test.zero", address=0x02000000, width=4)
                ])
                env.save_state()
                result = env.step(["A"], frames=2)
                self.assertEqual(result["frame"], initial["frame"] + 2)
                self.assertEqual(result["memory"]["test.zero"], 0)
                pixels = env.get_frame()
                self.assertEqual(len(pixels["pixels"]), 240 * 160 * 3)
                env.load_state()
                self.assertEqual(env.step(["A"], frames=2), result)
                self.assertEqual(env.get_frame(), pixels)
                with self.assertRaises(RuntimeError):
                    env.step(["invalid"])
                self.assertEqual(env.get_state(), result)
                with self.assertRaises(RuntimeError):
                    env.call("unknown")
                self.assertEqual(env.get_memory(0x02000000, 4), bytes(4))
                self.assertEqual(env.reset()["frame"], initial["frame"])


if __name__ == "__main__":
    unittest.main()
