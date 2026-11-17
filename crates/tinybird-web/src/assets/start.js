// The start page: whether this checkout is ready to play, read from
// /api/health, which reports what is on disk rather than what should be.

const $ = (id) => document.getElementById(id);

const BUILD_WASM = "cargo build -p tinybird-wasm --target wasm32-unknown-unknown --release";

function setServer(state, label) {
  $("link-state").dataset.state = state;
  $("link-label").textContent = label;
}

async function report() {
  let health;
  try {
    const response = await fetch("/api/health");
    health = await response.json();
  } catch {
    setServer("error", "server unreachable");
    $("setup-note").textContent = "unreachable";
    $("setup-lead").textContent = "The tinyBird server is not responding.";
    return;
  }
  setServer("live", "server up");

  const fixes = [];
  if (!health.wasm_ready) {
    $("setup-wasm").textContent = "not built";
    fixes.push(`Build the emulator module: ${BUILD_WASM}`);
  } else if (health.wasm_behind) {
    $("setup-wasm").textContent = `built, ${health.wasm_behind} older than its sources`;
    fixes.push(`Rebuild the emulator module: ${BUILD_WASM}`);
  } else {
    $("setup-wasm").textContent = "built and current";
  }

  // A missing BIOS is the difference between a correct picture and a corrupt
  // one in games that decompress graphics through BIOS calls.
  $("setup-bios").textContent = health.bios_ready ? "present" : "missing";
  if (!health.bios_ready) {
    fixes.push("Put a GBA BIOS dump at gba_bios.bin for full accuracy.");
  }

  if (!health.roms_served) {
    $("setup-roms").textContent = "not served (TINYBIRD_LOCAL_ROMS is off)";
  } else if (health.roms === 0) {
    $("setup-roms").textContent = `none in ${health.rom_dir}`;
    fixes.push(`Put .gba files in ${health.rom_dir}/, or open one from the player.`);
  } else {
    $("setup-roms").textContent = `${health.roms} in ${health.rom_dir}`;
  }

  $("setup-addons").textContent =
    health.addons === 1 ? "1 manifest" : `${health.addons} manifests`;

  $("setup-facts").hidden = false;
  const ready = health.wasm_ready && !health.wasm_behind;
  $("setup-note").textContent = ready ? "ready" : "needs a build";
  $("setup-lead").textContent = ready
    ? "Everything the player needs is in place."
    : "The player cannot boot until the emulator module is built.";
  if (fixes.length) {
    $("setup-fix").hidden = false;
    $("setup-fix").textContent = fixes.join(" ");
  }
}

report();
