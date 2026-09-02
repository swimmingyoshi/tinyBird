// The header every page outside the emulator shares: the account menu, and the
// dot that says whether the server is answering.
//
// Three pages had their own copy of the same six lines, each with its own
// wording for "server up". One copy, one wording.

import { mountAccount } from "/account.js";

/**
 * Wire up the bar. Returns the account controller, already refreshed.
 *
 * `onAccountChange` is passed straight through, for a page whose content
 * depends on who is signed in.
 */
export async function mountChrome({ onAccountChange } = {}) {
  const state = document.getElementById("link-state");
  const label = document.getElementById("link-label");

  // Asked before the account, because a server that is not answering makes
  // every other question on the page moot.
  try {
    const response = await fetch("/api/health");
    if (!response.ok) throw new Error(String(response.status));
    if (state) state.dataset.state = "live";
    if (label) label.textContent = "server up";
  } catch {
    if (state) state.dataset.state = "error";
    if (label) label.textContent = "server unreachable";
    // The menu is still mounted: it hides itself when it cannot ask who we
    // are, which is the same thing it does on a server with no accounts.
  }

  const accounts = mountAccount({ onChange: onAccountChange });
  await accounts?.refresh();
  return accounts;
}
