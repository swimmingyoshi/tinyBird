"""Dependency-free synchronous client for tinybird-headless (Python 3.9+)."""
import json
import subprocess
from pathlib import Path


class TinyBird:
    def __init__(self, rom, executable="tinybird-headless", bios=None, state=None, observations=None):
        args = [str(executable), str(rom)]
        for flag, path in (("--bios", bios), ("--state", state)):
            if path is not None:
                args.extend((flag, str(path)))
        self.process = subprocess.Popen(
            args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            text=True, encoding="utf-8", bufsize=1,
        )
        if observations is not None:
            try:
                self.configure_observations(observations)
            except Exception:
                self.close()
                raise

    def call(self, op, **arguments):
        if self.process.poll() is not None:
            raise RuntimeError("TinyBird process has exited")
        self.process.stdin.write(json.dumps(dict(op=op, **arguments)) + "\n")
        self.process.stdin.flush()
        line = self.process.stdout.readline()
        if not line:
            raise RuntimeError("TinyBird closed its output; see stderr for details")
        reply = json.loads(line)
        if not reply["ok"]:
            raise RuntimeError(reply["error"])
        return reply["result"]

    def reset(self):
        return self.call("reset")

    def step(self, buttons=(), frames=1):
        return self.call("step", buttons=list(buttons), frames=frames)

    def get_state(self):
        return self.call("get_state")

    def get_frame(self):
        return self.call("get_frame")

    def get_memory(self, address, length):
        return bytes(self.call("get_memory", address=address, length=length))

    def set_observations(self, fields):
        return self.call("set_observations", fields=fields)

    def configure_observations(self, config):
        """Apply a Workshop JSON file or dict after runtime compatibility checks."""
        if isinstance(config, (str, Path)):
            with open(config, encoding="utf-8") as source:
                config = json.load(source)
        return self.call("configure_observations", config=config)

    def save_state(self, slot=0):
        return self.call("save_state", slot=slot)

    def load_state(self, slot=0):
        return self.call("load_state", slot=slot)

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        finally:
            self.process.stdout.close()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
