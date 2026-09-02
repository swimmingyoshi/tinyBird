// The home page is a landing page: one claim, the three places it holds, and
// the way on. The live read-out it used to carry moved to /info, which is
// where somebody checking their own install is looking.
//
// So there is one thing left for script to decide.

import { mountChrome } from "/chrome.js";

/**
 * Show the contact signpost only if there is a form on the other end.
 *
 * The contact page says plainly when it is switched off, for somebody who
 * arrives by URL or by the nav. But sending them there from a card promising
 * to reach a person is a different thing, so this card waits to be told.
 */
async function signposts() {
  try {
    const state = await (await fetch("/api/contact")).json();
    document.getElementById("signpost-contact").hidden = !state.configured;
  } catch {
    // Unreachable server: the header dot already says so, and a hidden card
    // is the right failure here.
  }
}

mountChrome();
signposts();
