"""Read Workshop observations while controlling a ROM, using only the stdlib."""
import argparse
import json

from tinybird import TinyBird


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rom")
    parser.add_argument("observations", help="Workshop .observations.json export")
    parser.add_argument("--executable", default="tinybird-headless")
    parser.add_argument("--bios")
    parser.add_argument("--state")
    parser.add_argument("--steps", type=int, default=60)
    parser.add_argument("--frames", type=int, default=1)
    parser.add_argument("--buttons", nargs="*", default=[])
    args = parser.parse_args()
    if args.steps < 0 or not 1 <= args.frames <= 600:
        parser.error("steps must be nonnegative; frames must be between 1 and 600")
    with TinyBird(args.rom, executable=args.executable, bios=args.bios,
                  state=args.state, observations=args.observations) as env:
        print(json.dumps(env.reset()))
        for _ in range(args.steps):
            print(json.dumps(env.step(args.buttons, frames=args.frames)))


if __name__ == "__main__":
    main()
