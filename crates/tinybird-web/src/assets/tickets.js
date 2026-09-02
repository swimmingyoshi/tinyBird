// The ticket pages: everything you have sent, and one conversation at a time.
//
// Neither credential is here. The page asks this server, the server asks the
// contact service with the access token held in your session, and the service
// answers about the account that token names — see `crates/tinybird-web/src/
// contact.rs`. So there is nothing on this page to protect, and nothing it
// could show you that is not yours.
//
// The service has not published a schema for a ticket, so nothing here insists
// on one. Every field is read through `pick`, which takes the first name that
// is actually present: a listing that starts calling `status` `state` keeps
// rendering, and a field nobody anticipated is simply not drawn.

import { mountChrome } from "/chrome.js";

const $ = (id) => document.getElementById(id);

/** Where the list lives; a ticket is this plus its id. */
const ROOT = "/support/tickets";

/** What the server says about the form and the session. */
let panel = null;
/** Ceiling on a reply, from the same answer. */
let maxReply = 10000;
/** The ticket being shown, so a redraw after signing in returns to it. */
let showing = null;

// ------------------------------------------------------------------ plumbing

/**
 * The first of `names` this object actually has, as a trimmed string.
 *
 * The whole schema tolerance of this page in five lines. Nothing calls a
 * missing field an error, because a missing field is the ordinary case for an
 * API that has not promised which ones it sends.
 */
function pick(source, ...names) {
  for (const name of names) {
    const value = source?.[name];
    if (typeof value === "string" && value.trim()) return value.trim();
    if (typeof value === "number") return String(value);
  }
  return "";
}

/**
 * The array inside a paged answer, whatever it is called.
 *
 * A bare array, or the one array under a name that means "the things you asked
 * for". Anything else is no rows, which is drawn as empty rather than broken.
 */
function rows(payload) {
  if (Array.isArray(payload)) return payload;
  for (const name of ["tickets", "messages", "items", "results", "data", "records"]) {
    if (Array.isArray(payload?.[name])) return payload[name];
  }
  return [];
}

/** A timestamp in the reader's own locale, or nothing if it will not parse. */
function when(value) {
  if (!value) return "";
  const at = new Date(value);
  return Number.isNaN(at.getTime()) ? "" : at.toLocaleString();
}

/** Everything the panel can be showing, so one of them can be picked. */
const VIEWS = [
  "ticket-off",
  "ticket-accounts",
  "ticket-gate",
  "ticket-loading",
  "ticket-empty",
  "ticket-error",
  "ticket-list",
  "ticket-detail",
];

/** Show these, hide the rest. One state at a time, always exactly one. */
function show(...names) {
  for (const id of VIEWS) $(id).hidden = !names.includes(id);
}

/** The line beside the panel's heading. */
function note(words = "") {
  $("ticket-note").textContent = words;
}

function heading(words) {
  $("ticket-heading").textContent = words;
}

/** Show a failure with the button that tries it again. */
function fail(words, retry) {
  $("ticket-error-text").textContent = words;
  const button = $("ticket-retry");
  button.hidden = !retry;
  button.onclick = retry ?? null;
  show("ticket-error");
}

/**
 * Ask this server something, and turn the answers that are not data into the
 * one thing the caller has to handle.
 *
 * A lapsed session is the case worth separating: it arrives as an ordinary 401
 * in the middle of reading a page, and the answer to it is the sign-in panel,
 * not an error message about it.
 */
async function ask(path, options) {
  const response = await fetch(path, options);
  const body = await response.json().catch(() => ({}));
  if (response.status === 401) {
    const lapsed = new Error(body.error ?? "Sign in to read your tickets.");
    lapsed.lapsed = true;
    throw lapsed;
  }
  if (!response.ok) throw new Error(body.error ?? `The server answered ${response.status}.`);
  return body;
}

// --------------------------------------------------------------------- drafts
//
// An unsent reply and the key that will carry it, kept per ticket.
//
// The key is stored beside the draft rather than made at the moment of sending,
// and that is the point of it: a send that times out leaves both in place, so
// pressing the button again sends the same key and the service files one
// message rather than two. It is cleared only when a send has actually
// succeeded.

const DRAFT = (id) => `tinybird.ticket.draft.${id}`;
const KEY = (id) => `tinybird.ticket.key.${id}`;

/** Storage is unavailable in some private windows; a draft is not worth dying for. */
function remember(name, value) {
  try {
    if (value) localStorage.setItem(name, value);
    else localStorage.removeItem(name);
  } catch {
    /* nothing kept, everything still works */
  }
}

function recall(name) {
  try {
    return localStorage.getItem(name) ?? "";
  } catch {
    return "";
  }
}

/**
 * The idempotency key for this ticket's pending reply, made once and kept.
 *
 * `crypto.randomUUID` needs a secure context, which `localhost` is and a plain
 * `http://` on the network is not. The fallback is not a UUID and does not need
 * to be one — it only has to be unlikely to collide with somebody else's, and
 * the service is being told about one account's one reply.
 */
function replyKey(id) {
  const existing = recall(KEY(id));
  if (existing) return existing;
  const fresh =
    crypto.randomUUID?.() ??
    `tb-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`;
  remember(KEY(id), fresh);
  return fresh;
}

// ---------------------------------------------------------------------- list

/** One row: what it is about, where it has got to, and when it started. */
function ticketRow(ticket) {
  const id = pick(ticket, "id", "requestId", "ticket_id", "ticketId", "reference");
  const item = document.createElement("li");
  item.className = "tickets__row";

  const link = document.createElement("a");
  link.className = "tickets__link";
  link.href = `${ROOT}/${encodeURIComponent(id)}`;
  link.addEventListener("click", (event) => {
    // Left click only, and not when somebody meant a new tab.
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey) return;
    event.preventDefault();
    go(link.href);
  });

  const subject = document.createElement("span");
  subject.className = "tickets__subject";
  subject.textContent = pick(ticket, "subject", "title") || "(no subject)";
  link.append(subject);

  const status = pick(ticket, "status", "state");
  if (status) {
    const flag = document.createElement("span");
    flag.className = "tickets__state";
    flag.dataset.state = status.toLowerCase();
    flag.textContent = status;
    link.append(flag);
  }

  const stamp = when(pick(ticket, "createdAt", "created_at", "created", "openedAt"));
  const touched = when(pick(ticket, "updatedAt", "updated_at", "lastMessageAt"));
  const meta = document.createElement("span");
  meta.className = "tickets__meta";
  meta.textContent = [stamp && `opened ${stamp}`, touched && touched !== stamp && `last ${touched}`]
    .filter(Boolean)
    .join(" · ");
  link.append(meta);

  item.append(link);
  return item;
}

async function drawList() {
  showing = null;
  document.title = "tinyBird · Tickets";
  heading("Tickets");
  note("asking");
  show("ticket-loading");

  let payload;
  try {
    payload = await ask("/api/tickets");
  } catch (error) {
    if (error.lapsed) return gate();
    note("");
    return fail(error.message, drawList);
  }

  const tickets = rows(payload);
  if (!tickets.length) {
    note("none yet");
    return show("ticket-empty");
  }

  const list = $("ticket-list");
  list.replaceChildren(...tickets.map(ticketRow));
  note(tickets.length === 1 ? "1 ticket" : `${tickets.length} tickets`);
  show("ticket-list");
}

// -------------------------------------------------------------------- detail

/**
 * Which side of the conversation a message is on.
 *
 * Guessed, because the service names it differently in different places and
 * neither name is promised. An unrecognised message is left unattributed rather
 * than credited to the wrong person — a reply shown as your own is worse than
 * one shown as nobody's.
 */
function side(message) {
  const said = [
    pick(message, "direction", "role", "authorType", "author_type", "sender", "from", "author"),
    pick(message, "type"),
  ]
    .join(" ")
    .toLowerCase();
  if (/staff|operator|agent|support|admin|outbound|reply/.test(said)) return "them";
  if (/customer|user|requester|inbound|you/.test(said)) return "you";
  return "";
}

/** One message in the thread, as plain text and nothing else. */
function threadItem(message) {
  const item = document.createElement("li");
  item.className = "thread__item";
  const from = side(message);
  if (from) item.dataset.side = from;

  const head = document.createElement("p");
  head.className = "thread__head";

  const who = document.createElement("span");
  who.className = "thread__who";
  who.textContent =
    pick(message, "authorName", "author_name", "authorDisplayName", "name", "author") ||
    (from === "them" ? "Support" : from === "you" ? "You" : "Message");
  head.append(who);

  const stamp = when(pick(message, "createdAt", "created_at", "sentAt", "created", "timestamp"));
  if (stamp) {
    const at = document.createElement("time");
    at.className = "thread__when";
    at.textContent = stamp;
    head.append(at);
  }

  const body = document.createElement("p");
  body.className = "thread__body";
  // textContent, always. This is somebody else's writing arriving over the
  // network, and the only safe thing to do with it is show it as writing.
  body.textContent = pick(message, "message", "body", "text", "content") || "(empty)";

  item.append(head, body);
  return item;
}

/** The facts above the thread: state, when, what it was filed under. */
function drawFacts(ticket) {
  const facts = $("ticket-facts");
  facts.replaceChildren();
  const add = (label, value) => {
    if (!value) return;
    const row = document.createElement("div");
    const name = document.createElement("dt");
    name.textContent = label;
    const said = document.createElement("dd");
    said.textContent = value;
    row.append(name, said);
    facts.append(row);
  };

  add("State", pick(ticket, "status", "state"));
  add("About", pick(ticket, "category", "topic"));
  add("Opened", when(pick(ticket, "createdAt", "created_at", "created", "openedAt")));
  add("Last", when(pick(ticket, "updatedAt", "updated_at", "lastMessageAt")));
  add("Reference", pick(ticket, "id", "requestId", "ticket_id", "ticketId", "reference"));
}

/** The thread on its own, so a sent reply can redraw it without the rest. */
async function drawThread(id) {
  const payload = await ask(`/api/tickets/${encodeURIComponent(id)}/messages`);
  const messages = rows(payload);
  const thread = $("ticket-thread");
  if (!messages.length) {
    const empty = document.createElement("li");
    empty.className = "thread__item";
    empty.textContent = "No messages on this ticket yet.";
    thread.replaceChildren(empty);
    return;
  }
  thread.replaceChildren(...messages.map(threadItem));
}

async function drawTicket(id) {
  showing = id;
  heading("Ticket");
  note("asking");
  show("ticket-loading");

  let ticket;
  try {
    // Both at once: they are two calls to the same service about the same
    // thing, and waiting for one before starting the other only makes the page
    // slower to draw.
    [ticket] = await Promise.all([ask(`/api/tickets/${encodeURIComponent(id)}`), drawThread(id)]);
  } catch (error) {
    if (error.lapsed) return gate();
    note("");
    return fail(error.message, () => drawTicket(id));
  }

  const subject = pick(ticket, "subject", "title") || "(no subject)";
  $("ticket-subject").textContent = subject;
  document.title = `tinyBird · ${subject}`;
  drawFacts(ticket);

  const box = $("reply-message");
  box.maxLength = maxReply;
  box.value = recall(DRAFT(id));
  $("ticket-reply").hidden = false;
  sayReply(
    box.value
      ? "Unsent, kept from last time. It sends as one message however many times you press the button."
      : "Goes onto this ticket and reaches the same person by email.",
  );

  note(pick(ticket, "status", "state").toLowerCase());
  show("ticket-detail");
}

// --------------------------------------------------------------------- reply

function sayReply(words, tone = "") {
  const hint = $("reply-hint");
  hint.textContent = words;
  hint.dataset.tone = tone;
}

async function sendReply(event) {
  event.preventDefault();
  const id = showing;
  if (!id) return;

  const box = $("reply-message");
  const message = box.value.trim();
  if (!message) {
    sayReply("Nothing to send yet.", "bad");
    box.focus();
    return;
  }

  const button = $("reply-send");
  button.disabled = true;
  sayReply("Sending…");
  try {
    await ask(`/api/tickets/${encodeURIComponent(id)}/messages`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      // The key made when the draft was first sent, not one made now. A retry
      // after a timeout has to carry the same one or it is a second message.
      body: JSON.stringify({ message, idempotencyKey: replyKey(id) }),
    });

    // Sent, so the draft and its key are done: the next reply is a new
    // message and deserves a key of its own.
    remember(DRAFT(id), "");
    remember(KEY(id), "");
    box.value = "";
    sayReply("Sent.", "good");
    await drawThread(id);
  } catch (error) {
    if (error.lapsed) return gate();
    // The draft and the key both stay put, which is what makes pressing the
    // button again safe.
    sayReply(error.message, "bad");
  } finally {
    button.disabled = false;
  }
}

// ------------------------------------------------------------------- routing

/** The ticket named by the current path, if the path names one. */
function idFromPath() {
  if (!location.pathname.startsWith(ROOT)) return null;
  const rest = location.pathname.slice(ROOT.length).replace(/^\/+|\/+$/g, "");
  return rest ? decodeURIComponent(rest) : null;
}

/** Move to a ticket, or back to the list, without reloading the document. */
function go(href) {
  history.pushState(null, "", href);
  route();
}

function gate() {
  showing = null;
  note("sign in");
  show("ticket-gate");
}

/** Draw whatever the current path asks for. */
function route() {
  if (!panel?.configured) {
    note("off");
    return show("ticket-off");
  }
  // Tickets belong to accounts, so a server without them has no history to
  // show. Said in its own words rather than through the sign-in sentence: there
  // is no account menu to sign in from, and pointing at one would be a
  // dead end.
  if (!panel.requiresAccount) {
    showing = null;
    note("accounts off");
    return show("ticket-accounts");
  }
  if (!panel.signedIn) return gate();

  const id = idFromPath();
  return id ? drawTicket(id) : drawList();
}

async function render() {
  try {
    panel = await (await fetch("/api/contact")).json();
  } catch {
    note("");
    return fail("Could not reach this server.", render);
  }
  maxReply = panel.maxReply ?? maxReply;
  route();
}

async function start() {
  $("ticket-reply").addEventListener("submit", sendReply);

  // Kept on every keystroke rather than on leaving the page: `beforeunload` is
  // not fired reliably on mobile, and this is cheap.
  $("reply-message").addEventListener("input", (event) => {
    if (showing) remember(DRAFT(showing), event.target.value);
  });

  $("ticket-back").addEventListener("click", (event) => {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey) return;
    event.preventDefault();
    go(ROOT);
  });

  // Back and forward move between the list and a ticket, because they are two
  // pages as far as anyone reading them is concerned.
  addEventListener("popstate", () => {
    if (panel) route();
  });

  // Signing in or out changes everything this page is showing, so it is drawn
  // again rather than left claiming the last answer. The first draw is the
  // mount's own.
  const accounts = await mountChrome({ onAccountChange: () => render() });
  if (!accounts) await render();
}

start();
