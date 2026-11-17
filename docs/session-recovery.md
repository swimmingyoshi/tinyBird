# Session recovery

Open `/play` and choose **Continue playing** to restore the latest automatic
checkpoint. The card shows the game's filename, screenshot, and last-played time.
The ROM is stored locally too, so a game opened from your computer can resume
without selecting the file again or relying on an expired download link.

Recovery captures the machine state and cartridge save every 30 seconds while
playing, and on pause, tab hiding, game switching, and eject. Closing a tab
also attempts a final checkpoint, but browsers may terminate before that write
finishes. The most recent completed checkpoint remains available.

There is one automatic checkpoint in this browser. It is replaced atomically; a
failed write keeps the old one. Clearing browser storage removes recovery, and
recovery does not sync between devices. Use Save to file for saves you want to
keep independently.

Continue restores addon pane placement, selected tabs, split sizes, and the
game's Workshop observation configuration. Which add-ons are enabled still
follows your current preferences.
Resuming replaces the saved observation list for that game/revision with the
checkpoint's list.

Before restoring, tinyBird checks the recovery format, full ROM and state hashes,
game code, revision, and BIOS fingerprint. It then loads the state in a separate
emulator, which checks core state-format compatibility, before switching the
player to that machine. A failed validation leaves the checkpoint available and
shows the reason. Use the same BIOS as the original session.

## Verification

With the WASM emulator built and the existing Playwright installation available:

```sh
node tests/browser_recovery.mjs
```

The test uses a synthetic ROM, real WASM and isolated IndexedDB. It covers
periodic/pause capture, reload and resume, memory values, layout and observation
restoration, failed-write preservation, compatibility/corruption rejection, and
mobile layout.
