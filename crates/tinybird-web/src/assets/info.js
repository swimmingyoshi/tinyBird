// The info page is mostly prose, kept in step with PROGRESS.md by hand. The
// exception is the "This server" panel, which reports real state rather than
// claims: whether storage is configured, what is actually in the vault, and
// whether a BIOS dump is in place.
//
// It used to be on the home page. It belongs here: it is the answer to "is my
// install set up right", which is a question you ask about a thing you are
// already running, not one you ask at the front door.

import { mountChrome } from "/chrome.js";

const $ = (id) => document.getElementById(id);

async function reportStorage() {
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
    $("storage-note").textContent = "unreachable";
    $("storage-lead").textContent = "The tinyBird server is not responding.";
    return;
  }

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
  // one, so it is reported beside the vault rather than left to be discovered
  // when a game renders wrong.
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

mountChrome();
reportStorage();
