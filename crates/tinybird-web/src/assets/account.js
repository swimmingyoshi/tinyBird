// The account menu, shared by every page.
//
// It used to live in play.html's markup with its behaviour in play.js, which
// was fine while /play was the only page that had one. It is not: signing in
// is how you send a message, claim a save, or be someone in a room, so it
// belongs in the bar on every page rather than bolted to whichever form
// happens to need it.
//
// The page never sees a token. Signing in sets a first-party cookie this
// server issues, and the server holds what the auth service gave it; see
// `auth.rs` for why. So all this does is post a form and read back who it is
// talking to.
//
// The markup is built here rather than copied into four pages, so there is one
// place where the menu's shape is decided. Each page carries only:
//
//     <div class="account" id="account" hidden></div>

const MENU = `
  <button
    class="account__toggle"
    id="account-toggle"
    type="button"
    aria-expanded="false"
    aria-controls="account-menu"
  >
    <span class="account__dot" aria-hidden="true"></span>
    <span id="account-label">Sign in</span>
  </button>

  <div class="account__menu" id="account-menu" hidden>
    <div id="account-in" hidden>
      <p class="account__who" id="account-who">—</p>
      <div class="account__row">
        <button class="key key--slim" id="btn-signout" type="button">Sign out</button>
      </div>
      <!-- Where a page hangs its own buttons: /play puts "Claim old saves"
           here, and no other page has anything to add. -->
      <div class="account__row" id="account-extras"></div>
    </div>

    <!-- One form, two modes. Signing in wants two fields; creating an account
         wants four, and showing all four to someone who just wants to sign in
         is how a login form starts feeling like paperwork. \`data-mode\`
         decides which are on screen. -->
    <form id="account-form" class="account__form" data-mode="signin">
      <label class="account__field account__field--new">
        <span>Username</span>
        <input
          type="text"
          id="account-username"
          autocomplete="nickname"
          maxlength="32"
          spellcheck="false"
        />
      </label>
      <label class="account__field">
        <span>Email</span>
        <input type="email" id="account-email" autocomplete="username" required />
      </label>
      <label class="account__field">
        <span>Password</span>
        <input
          type="password"
          id="account-password"
          autocomplete="current-password"
          minlength="12"
          required
        />
      </label>
      <label class="account__field account__field--new">
        <span>Repeat password</span>
        <input
          type="password"
          id="account-password2"
          autocomplete="new-password"
          minlength="12"
        />
      </label>
      <div class="account__row">
        <button class="key key--slim" id="btn-signin" type="submit">Sign in</button>
        <button class="key key--slim" id="btn-signup" type="button">Create account</button>
        <button class="key key--slim" id="btn-signup-back" type="button" hidden>Back</button>
      </div>
      <p class="account__hint" id="account-hint">
        Saves are kept per account. 12 characters or more.
      </p>
    </form>
  </div>
`;

const $ = (id) => document.getElementById(id);

/**
 * Build the account menu into `#account` and wire it up.
 *
 * `onChange(user)` fires whenever the signed-in user changes, including the
 * first read on load and a sign-out — it is how a page reloads whatever
 * belonged to the last identity. Returns null when the page has no `#account`
 * host, so a page can opt out by leaving it out.
 */
export function mountAccount({ onChange } = {}) {
  const host = $("account");
  if (!host) return null;
  host.innerHTML = MENU;

  const el = {
    host,
    toggle: $("account-toggle"),
    menu: $("account-menu"),
    label: $("account-label"),
    signedIn: $("account-in"),
    who: $("account-who"),
    extras: $("account-extras"),
    form: $("account-form"),
    email: $("account-email"),
    password: $("account-password"),
    password2: $("account-password2"),
    username: $("account-username"),
    hint: $("account-hint"),
    signIn: $("btn-signin"),
    signUp: $("btn-signup"),
    signUpBack: $("btn-signup-back"),
    signOut: $("btn-signout"),
  };

  /** The signed-in user, or null. */
  let user = null;

  function say(text, tone) {
    el.hint.textContent = text;
    if (tone) el.hint.dataset.tone = tone;
    else delete el.hint.dataset.tone;
  }

  function showMenu(open) {
    el.menu.hidden = !open;
    el.toggle.setAttribute("aria-expanded", String(open));
    // Whichever control is the point of the menu right now.
    if (open) (user ? el.signOut : el.email).focus();
  }

  function render(next) {
    user = next;
    const on = Boolean(next);
    el.signedIn.hidden = !on;
    el.form.hidden = on;
    el.host.classList.toggle("is-signed-in", on);

    // The bar says whether you are signed in, not who you are: the address
    // appears only once the menu is open, so it is not sitting in a stream
    // capture.
    el.label.textContent = on ? "Account" : "Sign in";
    if (on) {
      el.who.textContent = next.display_name
        ? `${next.display_name} · ${next.email}`
        : next.email;
    }
    onChange?.(next);
  }

  function setMode(mode) {
    const creating = mode === "register";
    el.form.dataset.mode = mode;
    el.signIn.hidden = creating;
    el.signUpBack.hidden = !creating;
    el.signUp.textContent = creating ? "Create account" : "Create account…";
    el.password.autocomplete = creating ? "new-password" : "current-password";

    // Emptied on the way out rather than left filled and hidden: a repeated
    // password sitting in a hidden input is one the next person at this
    // browser can read out of the DOM.
    if (!creating) {
      el.username.value = "";
      el.password2.value = "";
    }
    say(
      creating
        ? "Pick a name others see in a room. 12 characters or more."
        : "Saves are kept per account. 12 characters or more.",
    );
  }

  async function submit(path, extra = {}) {
    const email = el.email.value.trim();
    const password = el.password.value;
    if (!email || !password) {
      say("Email and password, please.", "bad");
      return;
    }
    if (path.endsWith("register") && password !== el.password2.value) {
      say("Those two passwords are not the same.", "bad");
      return;
    }

    for (const button of [el.signIn, el.signUp]) button.disabled = true;
    say("Working…");
    try {
      const response = await fetch(path, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email, password, ...extra }),
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error ?? `${response.status}`);

      setMode("signin");
      // The password has done its job; no reason to leave it in a field a
      // screenshot or a shoulder would catch.
      el.password.value = "";
      el.password2.value = "";
      say("");
      render(body.user);
      showMenu(false);
    } catch (error) {
      say(error.message, "bad");
      for (const button of [el.signIn, el.signUp]) button.disabled = false;
      return;
    }
    for (const button of [el.signIn, el.signUp]) button.disabled = false;
  }

  /** Ask who we are. Also tells us whether accounts exist at all. */
  async function refresh() {
    try {
      const body = await (await fetch("/api/auth/me")).json();
      // Without a configured auth service the menu stays hidden and the page
      // falls back to whatever it does without accounts, which is a supported
      // way to run this.
      el.host.hidden = !body.configured;
      if (!body.configured) {
        render(null);
        return;
      }
      render(body.user ?? null);
    } catch {
      el.host.hidden = true;
    }
  }

  el.toggle.addEventListener("click", () => showMenu(el.menu.hidden));

  // A menu that will not close is worse than one that never opened.
  document.addEventListener("click", (event) => {
    if (!el.menu.hidden && !el.host.contains(event.target)) showMenu(false);
  });

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !el.menu.hidden) {
      showMenu(false);
      el.toggle.focus();
    }
  });

  el.form.addEventListener("submit", (event) => {
    event.preventDefault();
    // Enter in the create form means create, which is the button showing; in
    // the sign-in form it means sign in.
    if (el.form.dataset.mode === "register") el.signUp.click();
    else submit("/api/auth/login");
  });

  el.signUp.addEventListener("click", () => {
    // First click opens the longer form; the second submits it. Making an
    // account by accident out of a two-field box is worse than a click.
    if (el.form.dataset.mode !== "register") {
      setMode("register");
      el.username.focus();
      return;
    }
    submit("/api/auth/register", { displayName: el.username.value.trim() });
  });

  el.signUpBack.addEventListener("click", () => setMode("signin"));

  el.signOut.addEventListener("click", async () => {
    await fetch("/api/auth/logout", { method: "POST" });
    render(null);
    showMenu(false);
  });

  // Deliberately not refreshed here: the caller decides when to ask, because
  // on /play the answer reloads the vault and that has to happen in step with
  // the rest of startup rather than whenever this module happened to load.
  return {
    /** Re-ask the server who we are. */
    refresh,
    /** The signed-in user, or null. */
    get user() {
      return user;
    },
    /** Somewhere for a page to hang its own buttons in the signed-in block. */
    extras: el.extras,
    /**
     * Notice a session that ended without the page being told.
     *
     * Sessions do not survive a server restart, so a tab left open goes on
     * showing an account that no longer exists while every call answers 401.
     * A 401 where we believed we were signed in is the one signal we get.
     */
    noticeSignedOut() {
      if (!user) return;
      // Cleared before the re-ask so a second 401 behind this one does not
      // queue a second round trip.
      user = null;
      refresh();
    },
  };
}
