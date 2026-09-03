// The contact page.
//
// The form key is never here. This posts to /api/contact and the server does
// the talking; see crates/tinybird-web/src/contact.rs. Signing in is the bar's
// account menu, shared with every page — this only redraws when it changes.

import { mountChrome } from "/chrome.js";

const $ = (id) => document.getElementById(id);

/** The floor the server enforces, so the form can name it before sending. */
let minMessage = 10;
/** Whether the reply address comes from the session rather than the form. */
let signedIn = false;
/** The verified address Contact will acknowledge and support will answer. */
let replyAddress = "";

/** Say something under the Send button, in the tone the news deserves. */
function sayContact(words, tone = "") {
  const hint = $("contact-hint");
  hint.textContent = words;
  hint.dataset.tone = tone;
}

/**
 * Say it went, and offer the ticket it became.
 *
 * The link is the whole point of knowing the id: "it is on your tickets" is a
 * sentence somebody then has to act on, and this is the acting on it. Without
 * an id — an anonymous send, or a service that named none — the sentence stands
 * on its own, which is what it did before.
 */
function saySent(ticket, email) {
  sayContact(
    `Received. A confirmation is on its way${email ? ` to ${email}` : ""}. ` +
      "Support will respond as soon as possible. ",
    "good",
  );
  if (!ticket) return;

  const link = document.createElement("a");
  link.className = "contact__sent-link";
  link.href = `/support/tickets/${encodeURIComponent(ticket)}`;
  link.textContent = "See the ticket";
  $("contact-hint").append(link);
}

/**
 * Ask the server what the panel should be showing, and show it.
 *
 * Called again after signing in rather than reloading the page: the answer to
 * every question the panel asks — is there a key, is there a session, who are
 * you — lives in that one response, so re-reading it is the whole update.
 */
async function render() {
  let state = null;
  try {
    state = await (await fetch("/api/contact")).json();
  } catch {
    $("contact-note").textContent = "server unreachable";
    return false;
  }

  // On the home page this hid the panel. Here the panel is the page, so it
  // says why it is empty instead.
  if (!state.configured) {
    $("contact-off").hidden = false;
    $("contact-note").textContent = "off";
    return false;
  }

  minMessage = state.minMessage ?? minMessage;
  signedIn = Boolean(state.signedIn);
  replyAddress = signedIn && typeof state.email === "string" ? state.email : "";
  $("contact-message").minLength = minMessage;

  // Drawn before the gate check, because it is the one thing on this page
  // worth showing somebody whose session has just lapsed — it is where their
  // history is, once they sign back in.
  $("contact-tickets").hidden = !state.tickets;

  const gated = state.requiresAccount && !signedIn;
  $("contact-gate").hidden = !gated;
  $("contact-form").hidden = gated;
  $("contact-note").textContent = gated ? "sign in to send" : "";
  if (gated) return true;

  $("contact-as").hidden = !signedIn;
  // The address is the field that goes away, not the name: one comes from the
  // session, the other is still worth asking.
  $("contact-email-field").hidden = signedIn;
  if (signedIn) {
    $("contact-as").textContent = `Signed in as ${state.name} · ${state.email}`;
    // Prefilled, not fixed. Leaving it alone sends the username; typing over
    // it sends what you would rather be called, and the account travels with
    // the message either way.
    if (!$("contact-name").value) $("contact-name").value = state.name ?? "";
  }
  return true;
}

async function send() {
  const button = $("contact-send");
  const message = $("contact-message").value.trim();

  // The one rule worth checking here: it is the only one someone hits by
  // accident, and a round trip to say "too short" is a round trip wasted.
  if (message.length < minMessage) {
    sayContact(`A little more, please — at least ${minMessage} characters.`, "bad");
    $("contact-message").focus();
    return;
  }

  button.disabled = true;
  sayContact("Sending…");
  try {
    const response = await fetch("/api/contact", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        name: $("contact-name").value,
        // The address is left out when signed in: the server takes it from
        // the session, so sending one would only suggest it was used.
        ...(signedIn ? {} : { email: $("contact-email").value }),
        subject: $("contact-subject").value,
        message,
        category: $("contact-category").value,
        siteUrl: location.href,
        website: $("contact-website").value,
      }),
    });
    const body = await response.json().catch(() => ({}));

    // A session can lapse between loading the page and pressing Send. Say so
    // where the form was, rather than reporting it as a failure to deliver.
    if (response.status === 401) {
      $("contact-form").hidden = true;
      $("contact-gate").hidden = false;
      $("contact-note").textContent = "sign in to send";
      return;
    }
    if (!response.ok) throw new Error(body.error ?? `The server answered ${response.status}.`);

    // Cleared on the way out: leaving a sent message in the box invites a
    // second copy of it from whoever presses Send again.
    $("contact-subject").value = "";
    $("contact-message").value = "";
    $("contact-note").textContent = "sent";
    const email = signedIn ? replyAddress : $("contact-email").value.trim();
    saySent(signedIn && typeof body.ticket === "string" ? body.ticket : "", email);
  } catch (error) {
    sayContact(error.message, "bad");
  } finally {
    button.disabled = false;
  }
}

async function start() {
  $("contact-form").addEventListener("submit", (event) => {
    event.preventDefault();
    send();
  });

  // Signing in or out from the bar changes what this page should be showing,
  // so the panel is redrawn rather than left claiming the last answer. The
  // first redraw is the mount's own, which is what draws the panel at all.
  const accounts = await mountChrome({ onAccountChange: () => render() });
  // No account menu means no accounts on this server, so nothing has drawn
  // the panel yet.
  if (!accounts) await render();
}

start();
