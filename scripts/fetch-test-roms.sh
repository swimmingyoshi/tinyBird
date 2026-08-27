#!/usr/bin/env bash
#
# Fetch the GBA test ROMs the accuracy suite runs against.
#
#   ./scripts/fetch-test-roms.sh
#
# These are jsmolka's gba-tests: homebrew ROMs under the MIT licence, not
# commercial dumps. They are fetched rather than committed because `*.gba` is
# gitignored for good reason, and a checkout should not carry binaries it can
# download in a second.
#
# Without them `cargo test -p tinybird-core --test accuracy` reports every case
# as skipped rather than failing, so a fresh checkout still passes.
set -euo pipefail

REPO="https://raw.githubusercontent.com/jsmolka/gba-tests/master"
DEST="${1:-tests/roms}"

ROMS=(
  arm/arm.gba
  thumb/thumb.gba
  memory/memory.gba
  bios/bios.gba
  nes/nes.gba
  ppu/hello.gba
  ppu/shades.gba
  ppu/stripes.gba
  save/none.gba
  save/sram.gba
  save/flash64.gba
  save/flash128.gba
  unsafe/unsafe.gba
)

mkdir -p "$DEST"
printf 'Fetching %d test ROMs into %s\n' "${#ROMS[@]}" "$DEST"

for rom in "${ROMS[@]}"; do
  out="$DEST/$(basename "$(dirname "$rom")")-$(basename "$rom")"
  if curl -fsSL "$REPO/$rom" -o "$out"; then
    printf '  %-28s %s bytes\n' "$(basename "$out")" "$(wc -c <"$out" | tr -d ' ')"
  else
    printf '  %-28s FAILED\n' "$(basename "$out")" >&2
  fi
done

cat >"$DEST/README.md" <<'EOF'
# Test ROMs

Fetched by `scripts/fetch-test-roms.sh`, not committed.

These are [jsmolka/gba-tests](https://github.com/jsmolka/gba-tests), MIT
licensed homebrew. Each ROM runs a numbered series of checks and leaves the
result in `r12`: zero means every test passed, anything else is the number of
the first one that failed. On failure it also writes the decimal digits to
IWRAM at `0x03000000`.

Run them with:

    cargo test -p tinybird-core --test accuracy -- --nocapture
EOF

printf 'Done. Run: cargo test -p tinybird-core --test accuracy -- --nocapture\n'
