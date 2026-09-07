# Cartridge clock (Ruby, Sapphire, Emerald)

The cartridge RTC now drives each response bit on falling SCK and holds it
through rising SCK, when Pokémon reads GPIO. Previously the rising edge advanced
to the next bit too early, corrupting status and BCD dates. This could trigger
the internal battery warning and prevent time-based events.

The protocol matches the sampling sequence in
[Pokémon's SIIRTC driver](https://github.com/pret/pokeemerald/blob/master/src/siirtc.c)
and the edge behavior documented in
[mGBA's cartridge GPIO implementation](https://github.com/mgba-emu/mgba/blob/master/src/gba/cart/gpio.c).

Single-player Play supplies current host time before running frames, including
after resuming a suspended tab. Link sessions retain their synchronized clock.
The RTC tests cover stable response bits, healthy battery status, BCD dates,
emulated elapsed time, and a host clock update after two offline days.

To try the fix, save inside Pokémon, refresh the webapp to load the new WASM,
then reset the game and choose Continue. An old emulator savestate can retain
the game's previously detected battery error in RAM; rebooting makes the game
probe the corrected clock again. Existing save files are not rewritten to
change event timestamps. In-game berry growth on existing user saves still
needs manual verification.

Run `cargo test -p tinybird-core --lib` for the regression suite.
