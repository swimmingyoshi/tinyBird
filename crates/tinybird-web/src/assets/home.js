// The home page shows real state rather than claims: whether storage is
// configured and what is actually in the vault.

const $ = (id) => document.getElementById(id);

async function report() {
  const link = $("link-state");
  const label = $("link-label");

  let library = null;
  let local = [];
  try {
    const [libraryResponse, localResponse] = await Promise.all([
      fetch("/api/library"),
      fetch("/api/local"),
    ]);
    library = await libraryResponse.json();
    local = (await localResponse.json()).assets ?? [];
  } catch {
    link.dataset.state = "error";
    label.textContent = "server unreachable";
    $("storage-note").textContent = "unreachable";
    $("storage-lead").textContent = "The tinyBird server is not responding.";
    return;
  }

  link.dataset.state = "live";
  label.textContent = "server up";

  const romCount =
    (library.assets ?? []).filter((a) => /\.(gba|bin|agb)$/i.test(a.name)).length +
    local.length;

  if (!library.configured) {
    $("storage-note").textContent = "off";
    $("storage-lead").textContent =
      "No vault configured. Set TINYBIRD_MEDIA_KEY in .env to list and store assets on your media host.";
  } else if (library.error) {
    $("storage-note").textContent = "error";
    $("storage-lead").textContent = library.error;
  } else {
    $("storage-note").textContent = "connected";
    $("storage-lead").textContent =
      "Your vault is reachable. ROMs listed here can be opened in the browser.";
  }

  // A missing BIOS is the difference between a correct picture and a corrupt
  // one, so it belongs next to the storage state rather than buried.
  let biosOk = false;
  try {
    biosOk = (await fetch("/bios", { method: "HEAD" })).ok;
  } catch {
    biosOk = false;
  }

  $("storage-facts").hidden = false;
  $("storage-bios").textContent = biosOk ? "loaded" : "missing";
  $("storage-vault").textContent = library.configured ? library.vault : "not configured";
  $("storage-roms").textContent =
    romCount === 0 ? "none found" : `${romCount} ready to play`;
}

report();
