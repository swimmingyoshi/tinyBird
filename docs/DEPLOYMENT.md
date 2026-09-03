# Production deployment

The production shape is one `tinybird-web` container behind the existing
Cloudflare Tunnel on the VPS. Do not publish the container port to the host;
attach it to the Docker network used by `cloudflared` and route the hostname to
`http://tinybird-web:8877`.

One replica is deliberate. Sessions, multiplayer rooms, and contact throttles
live in process memory. A restart signs users out and clears rooms; multiple
replicas would disagree unless that state moves to a shared store.

## Prepare the host

```bash
git clone https://github.com/swimmingyoshi/tinyBird.git
cd tinyBird
cp deploy/tinybird.env.example deploy/tinybird.env
chmod 600 deploy/tinybird.env
```

Populate these server-side secrets in `deploy/tinybird.env`:

- `TINYBIRD_AUTH_PROJECT_SECRET`
- `TINYBIRD_MEDIA_KEY`
- `TINYBIRD_CONTACT_KEY`

The image and build context exclude `.env` files, ROMs, BIOS images, saves, and
local runtime data. The production Compose file also forces
`TINYBIRD_LOCAL_ROMS=off`; do not override it on an internet-facing host.

## Connect it to the tunnel

Find the network containing the existing `cloudflared` container, then supply
its name without editing the Compose file:

```bash
docker inspect cloudflared --format '{{range $name, $_ := .NetworkSettings.Networks}}{{$name}}{{"\n"}}{{end}}'
export TINYBIRD_TUNNEL_NETWORK=<network-name>
docker compose -f compose.production.yaml config
docker compose -f compose.production.yaml build
docker compose -f compose.production.yaml up -d
```

Configure the existing tunnel's published application for:

```yaml
- hostname: gba.0xstash.dev
  service: http://tinybird-web:8877
```

Keep the tunnel's existing final catch-all rule. Validate a locally-managed
configuration with `cloudflared tunnel ingress validate` before reloading it.

## Verify before portal cutover

```bash
docker compose -f compose.production.yaml ps
docker compose -f compose.production.yaml logs --tail=100 tinybird-web
curl --fail https://gba.0xstash.dev/api/health
curl --fail https://gba.0xstash.dev/api/library
curl --fail https://gba.0xstash.dev/api/auth/me
curl --fail https://gba.0xstash.dev/api/contact
```

Also verify `/tinybird.wasm`, `/play`, and a WebSocket lobby connection. Only
after authenticated ticket isolation and reply idempotency pass should Contact
switch its customer portal from hosted mode to `gba.0xstash.dev`.

The container intentionally carries no GBA BIOS. Players can use the emulator's
HLE behavior; distributing a BIOS or ROM in the image is out of scope.
