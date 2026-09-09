# One repository, three modes

Keep shared code on `main`. Use short-lived feature/release-preparation branches
and reviewed pull requests. Mark tested checkpoints with tags when releasing.
Do not maintain a local fork or a second copied website: those drift and need
every bug/security fix applied twice. A Git worktree is useful for running two
checkouts side by side; it is a development convenience, not a security boundary.

| Mode | Purpose | Services and data |
| --- | --- | --- |
| `local` (default) | Downloadable personal edition | Loopback only; no cloud credentials, accounts, publishing, support, or lobbies. Browser files/recovery and local Workshop remain available. |
| `development` | Contributor testing | Loopback only; explicitly reads `.env.development`. Use separate test service projects and data. |
| `production` | Public site and production-like staging | Explicit HTTPS public origin, injected secrets, account ownership for hosted storage/contact, no local ROM/BIOS/snapshot routes. |

Set `TINYBIRD_MODE` or pass `--mode`. Unknown modes fail startup. Production and
local mode do not read `.env` files. Local ignores service credentials even if
inherited from the environment and uses an in-memory community database. Local
JSON addons and observations remain in browser storage.

## Run from source

```sh
rustup target add wasm32-unknown-unknown
cargo build --locked --release -p tinybird-wasm --target wasm32-unknown-unknown
cargo run --locked -p tinybird-web -- --mode local
```

For contributor service testing, copy `.env.example` to `.env.development`, set
test-only credentials and `TINYBIRD_WEB_PORT=8878`, then:

```sh
cargo run --locked -p tinybird-web -- --mode development
```

Use separate origins/ports and database locations for local and development so
browser storage, cookies, and server data do not mix. Never reuse production
service keys, live user databases, or production cookies in development.

## Public deployment

`compose.production.yaml` explicitly selects production mode. Populate
`deploy/tinybird.env` outside Git, including `TINYBIRD_PUBLIC_ORIGIN` (an HTTPS
origin such as `https://gba.0xstash.dev`). A staging deployment uses the same
production mode but its own hostname, environment secrets, service projects,
database volume, and tunnel. Mode selection alone does not isolate infrastructure.

The public server must remain behind the TLS tunnel/reverse proxy, with its
backend port inaccessible publicly. Production forces Secure session cookies,
checks exact request origins on mutations and WebSocket upgrades, adds baseline
security headers, removes filesystem paths from public health output, and blocks
the unauthenticated shared-library upload endpoint. There is no public replacement
for that endpoint yet; provision a shared library through trusted operator tools.
Command-line API clients must send the configured Origin on mutations as well.
Origin validation supplements authentication; it does not authenticate scripts.

Users choose BIOS files in Play's Audio & video menu. They remain browser-local.
Production never serves the host's BIOS, ROM folder, or desktop snapshot.

## Distribute the local edition

```sh
cargo build --locked --release -p tinybird-web -p tinybird-runtime
cargo build --locked --release -p tinybird-wasm --target wasm32-unknown-unknown
python scripts/package-local.py
```

The package is assembled from an allowlist into `target/dist`: executables,
WASM, shipped JSON addons, Python helpers, license and user docs. It never copies
the working tree, `.env`, personal ROMs/BIOS, databases, server data or deployment
files. The shared web executable still contains hosted code, disabled by the
local mode. Compile-time feature separation can be added if binary size or
dependency removal becomes a goal; it is not necessary to fork the repository.

The manual **Package local edition** workflow builds Windows, Linux, and macOS
archives without production secrets. Download and verify its artifacts, then
attach the ZIPs and SHA-256 files to a GitHub Release. This makes the local edition
available without giving users source/build tools. The workflow itself does not
publish a release or deploy the website. Platform signing/notarization is a
follow-up; these packages are unsigned. See [GitHub releases](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases).

## Push and release gates

1. Run `python scripts/check-release.py` and `git diff --check`.
2. Run Rust/web/runtime/addon tests, JS module tests, Python client tests, the
   recovery/Workshop browser regressions, and `tests/deployment_smoke.py` against
   the newly built executable. CI includes the source tests; browser integration
   coverage still requires its Playwright installation.
3. Commit a coherent checkpoint on the preparation branch. Push that branch and
   open a PR to `main`; do not deploy a dirty working tree.
4. Deploy the reviewed commit to isolated staging, test accounts and save
   ownership with two users, and test addon publishing/moderation and WebSockets
   through the actual proxy. Promote that exact image digest to production.
5. Back up the production SQLite database using SQLite's backup API before a
   release. Keep the previous image digest and compatible backup for rollback.

Use a protected production GitHub Environment for deployment secrets and branch
restrictions. No production deployment workflow is introduced in this checkpoint.
See [GitHub environment controls](https://docs.github.com/en/actions/concepts/workflows-and-actions/deployment-environments).

## Security work still required before calling this production-audited

- Review public authentication/registration and WebSocket resource/rate limits,
  including protections at the actual edge proxy.
- Audit dependency advisories and repository history with a dedicated secret
  scanner; the included check only catches payload files, private-key markers
  and current configured secrets in candidate files, not every possible secret.
- Exercise cross-account save/screenshot ownership, token revocation, addon
  moderation, and storage proxy/redirect behavior against isolated real services.
- Expand the baseline CSP to a tested script/style/connect allowlist; the current
  policy restricts framing, base URLs and objects but is not a full XSS policy.
- Check production logs, retention, backup restoration and incident response.

These changes establish safer defaults and testable boundaries; they are not a
full security audit. The request-origin policy follows the exact-origin approach
described by [OWASP](https://cheatsheetseries.owasp.org/cheatsheets/Cross-Site_Request_Forgery_Prevention_Cheat_Sheet.html).
