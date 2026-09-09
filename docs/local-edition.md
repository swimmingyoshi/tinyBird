# tinyBird Local

Extract this archive into a folder you can write to. Run `start-local.cmd` on
Windows, or `./start-local.sh` on Linux/macOS, then open
<http://127.0.0.1:8877/play>. Keep that terminal open while playing. On macOS/Linux
you may need `chmod +x start-local.sh tinybird-web tinybird-headless` after extraction.

This edition needs no account, API key, cloud database, or Docker. It includes
the browser emulator, built-in readers, local addon import/editing, Workshop,
observation export, automatic recovery, screenshot downloads, and the headless
Python interface. Python automation requires Python installed separately.

Choose your own game file in Play. Choose your BIOS under **Menu → Audio & video**
before loading a game; it stays in the browser. ROMs and BIOS files are not
included. You can alternatively place ROMs in a `roms` subfolder and a BIOS at
`gba_bios.bin`. Those files are served only by the loopback local server.

Save to file keeps a named checkpoint you can load again. Automatic recovery is
separate, in this browser. Use the same hostname and port to access that browser
storage on your next launch. Clearing browser data removes recovery, local
readers, observations, and your browser-stored BIOS.

Cloud Vault, accounts, publishing/moderation, contact/support, and network lobbies
are disabled in this edition. Local addon JSON import/export still works. The
same executable contains shared site code; these are enforced runtime boundaries,
not a claim that hosted code has been removed from the executable.

The emulator and local tools work without hosted services. Optional web fonts
and uncached species artwork may be unavailable offline; system fonts and text
remain usable. Do not expose this loopback edition through a public tunnel.

See `automation.md` for Python and `session-recovery.md` for recovery details.
