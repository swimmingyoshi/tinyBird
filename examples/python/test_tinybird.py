"""End-to-end protocol/client regression using a synthetic ARM ROM."""
from pathlib import Path
import os
import tempfile
import unittest

from tinybird import TinyBird


class RuntimeTest(unittest.TestCase):
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
