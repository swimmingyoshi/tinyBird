//! Two Pokémon carts on one cable, from savestates taken at the trade counter.
//!
//! Ignored by default: it needs a commercial ROM and two savestates that
//! cannot be committed. Point it at them and run:
//!
//! ```text
//! cargo test --release -p tinybird-core --test link_trade -- --ignored --nocapture
//! ```
//!
//! This is the test a browser cannot be: no network, no round trips, no timing
//! of its own — just two consoles wired together and stepped in lockstep. If a
//! trade works here and not in a browser, the fault is in the transport. If it
//! fails here, the fault is in the serial hardware.
//!
//! Nobody is holding the consoles, so the test has to press the buttons, and
//! which presses get two games from "standing at the counter" to "trading" is
//! not something to guess at. `find_the_input_that_starts_a_trade` tries
//! several and reports how far each got, writing the screens out as a
//! filmstrip to look at.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tinybird_core::bus::Bus;
use tinybird_core::gba::{link_step, Gba};
use tinybird_core::sio::{PortMode, SerialMode};
use tinybird_core::GbaButton;

const ROM: &str = "../../roms/pokemon_fire_red.gba";
const STATE_A: &str = "../../TradeTest1.state";
const STATE_B: &str = "../../TradeTest2.state";

const RCNT: u32 = 0x0400_0134;
const SIOCNT: u32 = 0x0400_0128;
const SIOMLT_SEND: u32 = 0x0400_012A;

fn path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn console(state: &str, seat: u8) -> Option<Gba> {
    let rom = std::fs::read(path(ROM)).ok()?;
    let state = std::fs::read(path(state)).ok()?;

    let mut gba = Gba::new();
    gba.load_rom(rom);
    gba.load_state_bytes(&state)
        .expect("the savestate should load against this cartridge");
    gba.start();
    gba.link_connect(seat, 2);
    Some(gba)
}

/// How the serial port is set up right now, in words.
fn port(gba: &Gba) -> String {
    let rcnt = gba.bus.read_u16(RCNT);
    let siocnt = gba.bus.read_u16(SIOCNT);
    format!(
        "SIOCNT={siocnt:04X}/{:?} rcnt={:?} baud={} si={} sd={} id={} start={} irq={}",
        SerialMode::from_control(siocnt),
        PortMode::from_rcnt(rcnt),
        siocnt & 0b11,
        (siocnt >> 2) & 1,
        (siocnt >> 3) & 1,
        (siocnt >> 4) & 0b11,
        (siocnt >> 7) & 1,
        (siocnt >> 14) & 1,
    )
}

/// What to hold on each console at a given frame.
type Script = fn(u32) -> (GbaButton, GbaButton);

fn beat(frame: u32, period: u32, held: u32) -> GbaButton {
    if frame % period < held {
        GbaButton::A
    } else {
        GbaButton::empty()
    }
}

/// Press A on both, on a cadence slow enough for a dialogue box to finish.
fn both_together(frame: u32) -> (GbaButton, GbaButton) {
    let a = beat(frame, 40, 6);
    (a, a)
}

/// The same, but the second console is half a beat behind: two people do not
/// press their buttons on the same frame.
fn child_offset(frame: u32) -> (GbaButton, GbaButton) {
    (beat(frame, 40, 6), beat(frame + 20, 40, 6))
}

/// Slower, in case a beat of six frames is skipping past a menu.
fn slow(frame: u32) -> (GbaButton, GbaButton) {
    let a = beat(frame, 90, 8);
    (a, a)
}

/// Walk up first. If a savestate is a step short of the counter, nothing else
/// in here can ever work.
fn walk_up_then_talk(frame: u32) -> (GbaButton, GbaButton) {
    if frame < 60 {
        return (GbaButton::UP, GbaButton::UP);
    }
    both_together(frame - 60)
}

/// Only the first console talks to its attendant, to see whether a console
/// left alone answers a link on its own.
fn parent_only(frame: u32) -> (GbaButton, GbaButton) {
    (beat(frame, 40, 6), GbaButton::empty())
}

/// Where FireRed keeps the player's party, and how big a slot is.
const PARTY_BASE: u32 = 0x0202_4284;
const PARTY_SLOT: u32 = 100;
const PARTY_SLOTS: u32 = 6;

/// Who is in a party right now, by the two fields that identify a Pokemon.
///
/// Personality value and original-trainer id, at offsets 0 and 4 of a slot.
/// Both are **unencrypted** — the rest of the slot is XOR-scrambled with a key
/// derived from these two — so a trade is detectable without decrypting
/// anything or knowing what any of it means.
///
/// That is the whole trick: "did a Pokemon change hands" stops being a
/// judgement call about a filmstrip and becomes a set comparison.
fn party_ids(gba: &Gba) -> Vec<(u32, u32)> {
    (0..PARTY_SLOTS)
        .map(|slot| {
            let at = PARTY_BASE + slot * PARTY_SLOT;
            (gba.read_u32(at), gba.read_u32(at + 4))
        })
        // A personality of zero is an empty slot, not a Pokemon.
        .filter(|(personality, _)| *personality != 0)
        .collect()
}

/// What changed hands between two consoles, if anything.
#[derive(Debug, Default, PartialEq, Eq)]
struct Exchange {
    /// Pokemon the parent has now that the child had before.
    parent_received: usize,
    /// And the other way.
    child_received: usize,
    /// Either party changed at all, trade or not — a level-up or an item
    /// would do this too, so it is a hint rather than a verdict.
    parties_changed: bool,
}

impl Exchange {
    fn traded(&self) -> bool {
        self.parent_received > 0 && self.child_received > 0
    }
}

fn exchange(
    before: (&[(u32, u32)], &[(u32, u32)]),
    after: (&[(u32, u32)], &[(u32, u32)]),
) -> Exchange {
    let (parent_before, child_before) = before;
    let (parent_after, child_after) = after;

    let gained = |now: &[(u32, u32)], had: &[(u32, u32)], theirs: &[(u32, u32)]| {
        now.iter()
            .filter(|id| !had.contains(id) && theirs.contains(id))
            .count()
    };

    Exchange {
        parent_received: gained(parent_after, parent_before, child_before),
        child_received: gained(child_after, child_before, parent_before),
        parties_changed: parent_after != parent_before || child_after != child_before,
    }
}

/// What a run of one strategy came to.
struct Outcome {
    transfers: usize,
    /// Distinct halfwords each console put on the wire. One is a console that
    /// never joined in; many is a console holding a conversation.
    parent_values: usize,
    child_values: usize,
    ports: String,
    /// Whether anything actually changed hands. This is the one that matters:
    /// everything above says the cable is working, and only this says the
    /// trade completed.
    exchange: Exchange,
}

fn run(script: Script, seconds: u32, film: Option<&str>) -> Option<Outcome> {
    run_from(script, seconds, film, 0, true)
}

/// The system clock, for turning milliseconds of network into cycles.
const CLOCK_HZ: f64 = 16_777_216.0;

fn run_from(
    script: Script,
    seconds: u32,
    film: Option<&str>,
    film_from: u32,
    cabled: bool,
) -> Option<Outcome> {
    run_with_latency(script, seconds, film, film_from, cabled, 0.0)
}

/// The same, with `latency_ms` of network between the consoles.
///
/// Every transfer is held for a round trip before it lands, which is the one
/// thing the browser has and this harness does not. Everything else — the
/// frame barrier, the transfer taking the parent's cycles — the harness
/// already matches, so sweeping this says how much round trip a real trade
/// will put up with.
fn run_with_latency(
    script: Script,
    seconds: u32,
    film: Option<&str>,
    film_from: u32,
    cabled: bool,
    latency_ms: f64,
) -> Option<Outcome> {
    run_with_jitter(script, seconds, film, film_from, cabled, latency_ms, 0.0, 0)
}

/// The same, with `spike_ms` every `spike_every` transfers.
///
/// A network is not one number. Between two browsers on one machine the round
/// trip averaged a millisecond and its worst was seventeen, so what matters is
/// not the average but whether the occasional bad one is survivable.
#[allow(clippy::too_many_arguments)]
fn run_with_jitter(
    script: Script,
    seconds: u32,
    film: Option<&str>,
    film_from: u32,
    cabled: bool,
    latency_ms: f64,
    spike_ms: f64,
    spike_every: usize,
) -> Option<Outcome> {
    let mut a = console(STATE_A, 0)?;
    let mut b = console(STATE_B, 1).expect("the second console");
    if !cabled {
        // The control: same games, same buttons, no cable between them.
        a.link_disconnect();
        b.link_disconnect();
    }
    let mut consoles = vec![a, b];
    let mut targets: Vec<u64> = consoles.iter().map(|c| c.frame_count).collect();

    let parties_before = (party_ids(&consoles[0]), party_ids(&consoles[1]));

    let mut transfers = 0usize;
    let mut parent_seen = HashSet::new();
    let mut child_seen = HashSet::new();
    let delay_cycles = (latency_ms / 1000.0 * CLOCK_HZ) as u64;
    let spike_cycles = (spike_ms / 1000.0 * CLOCK_HZ) as u64;
    let mut frames = Vec::new();
    let mut last_look = (0u64, 0u64);
    let mut changes = 0usize;
    let total = seconds * 60;
    // Spread the samples over whatever part of the run is interesting, which
    // for a long one is the end rather than the menus at the start.
    let every = ((total.saturating_sub(film_from)) / 6).max(1);

    for frame in 0..total {
        for target in targets.iter_mut() {
            *target += 1;
        }

        let (press_a, press_b) = script(frame);
        consoles[0].input.set_buttons(press_a);
        consoles[1].input.set_buttons(press_b);

        let mut guard = 0u32;
        loop {
            let mut running = false;
            for (index, console) in consoles.iter_mut().enumerate() {
                if console.frame_count < targets[index] {
                    console.step();
                    running = true;
                }
            }

            if consoles[0].link_transfer_pending() {
                // The parent stops the instant it asks, and the page freezes
                // it until the answer arrives, so no emulated time passes
                // there however slow the network is. That is what the freeze
                // is for.
                //
                // The child is the gap. It has not heard yet, and keeps
                // running until the message reaches it — one way, not a round
                // trip. Whatever it does in that window is time the two
                // consoles did not spend together.
                let lead = if spike_every > 0 && transfers > 0 && transfers % spike_every == 0 {
                    spike_cycles
                } else {
                    delay_cycles
                };
                if lead > 0 {
                    let until = consoles[1].total_cycles + lead;
                    while consoles[1].total_cycles < until && !consoles[1].link_transfer_pending() {
                        consoles[1].step();
                    }
                }

                let sent = (
                    consoles[0].bus.read_u16(SIOMLT_SEND),
                    consoles[1].bus.read_u16(SIOMLT_SEND),
                );
                if link_step(&mut consoles) {
                    transfers += 1;
                    parent_seen.insert(sent.0);
                    child_seen.insert(sent.1);
                }
            }

            guard += 1;
            if !running || guard > 4_000_000 {
                break;
            }
        }

        if film.is_some() && frame >= film_from && frames.len() < 12 {
            let now = (fingerprint(&consoles[0]), fingerprint(&consoles[1]));
            // The first sample always, then only when a screen actually
            // changes. A still picture repeated six times says nothing; a menu
            // that opens for a moment is what needs catching.
            if frames.is_empty() || now != last_look {
                last_look = now;
                for console in consoles.iter() {
                    frames.push(screen(console));
                }
                changes += 1;
            }
        }
        let _ = every;
    }

    if let Some(name) = film {
        write_film(name, &frames, 2);
        println!("    screens changed {changes} times while filming");
    }

    let parties_after = (party_ids(&consoles[0]), party_ids(&consoles[1]));

    Some(Outcome {
        transfers,
        parent_values: parent_seen.len(),
        child_values: child_seen.len(),
        ports: format!("{} | {}", port(&consoles[0]), port(&consoles[1])),
        exchange: exchange(
            (&parties_before.0, &parties_before.1),
            (&parties_after.0, &parties_after.1),
        ),
    })
}

/// A cheap summary of what is on screen, for spotting change.
fn fingerprint(gba: &Gba) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    // Every 37th pixel: enough to notice a dialogue box opening, cheap enough
    // to do on every frame of a long run.
    for pixel in gba.ppu.get_framebuffer().as_slice().iter().step_by(37) {
        hash ^= u64::from(pixel.color.to_rgb555());
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// One screen as RGB bytes.
fn screen(gba: &Gba) -> Vec<u8> {
    let mut out = Vec::with_capacity(240 * 160 * 3);
    for pixel in gba.ppu.get_framebuffer().as_slice() {
        let rgb = pixel.color.to_rgb555();
        let widen = |five: u16| ((five as u32 * 255) / 31) as u8;
        out.push(widen(rgb & 0x1F));
        out.push(widen((rgb >> 5) & 0x1F));
        out.push(widen((rgb >> 10) & 0x1F));
    }
    out
}

/// Lay the screens out as a grid so a whole run can be looked at in one go:
/// the two consoles above one another, time running left to right.
fn write_film(name: &str, frames: &[Vec<u8>], rows: usize) {
    if frames.is_empty() {
        return;
    }
    let columns = frames.len().div_ceil(rows);
    let (w, h) = (240usize, 160usize);
    let mut out = format!("P6\n{} {}\n255\n", w * columns, h * rows).into_bytes();

    for row in 0..rows {
        for y in 0..h {
            for column in 0..columns {
                match frames.get(column * rows + row) {
                    Some(frame) => {
                        let start = y * w * 3;
                        out.extend_from_slice(&frame[start..start + w * 3]);
                    }
                    None => out.extend(std::iter::repeat(0u8).take(w * 3)),
                }
            }
        }
    }

    let file = path(&format!("../../link-film-{name}.ppm"));
    std::fs::write(&file, out).expect("write the filmstrip");
    println!("    filmstrip -> link-film-{name}.ppm");
}

/// Everything from standing at the counter to sitting at the trade table.
///
/// Worked out from the filmstrips: talking to the attendant is a run of A
/// presses, and once both games say "Please enter." the players have to walk
/// north into the Trade Center and sit down.
fn full_trade(frame: u32) -> (GbaButton, GbaButton) {
    // Talk to the attendant, agree to save, confirm the link.
    if frame < 820 {
        return both_together(frame);
    }

    // Then walk north and knock on whatever is in the way, over and over: into
    // the Trade Center, up to the table, and A at the seat. Alternating beats
    // a fixed route because it does not need to know how many steps anything
    // is, only that walking and talking both keep happening.
    // Both walk north through the door into the Trade Center.
    if frame < 1000 {
        return (GbaButton::UP, GbaButton::UP);
    }

    // Step apart, so each stands at its own terminal rather than both
    // crowding the same one.
    if frame < 1060 {
        return (GbaButton::LEFT, GbaButton::RIGHT);
    }

    // Up to the machine.
    if frame < 1160 {
        return (GbaButton::UP, GbaButton::UP);
    }

    // And talk to it.
    both_together(frame)
}

/// Two real cartridges, linked, from the counter to the Trade Center.
///
/// What this establishes, with a commercial game rather than a fixture:
///
/// - the handshake completes — the save prompt, "Start link with 2 players",
///   and the attendant saying "Please enter";
/// - both players walk into the same Trade Center and stand at the machine;
/// - each console draws the *other* player and follows them as they move,
///   which is position data crossing the cable and nothing else;
/// - thousands of transfers carry real, varied traffic in both directions.
///
/// It stops short of an actual Pokémon changing hands. Opening the trade menu
/// needs a button pressed on an exact tile, and that is a puppeteering problem
/// rather than an emulation one: the games are demonstrably running, taking
/// input and talking to each other at that point.
///
/// The control run is the point of the numbers. The same games given the same
/// buttons with no cable between them have to come out clearly different, or
/// the measurement is not measuring anything.
#[test]
#[ignore = "needs a commercial ROM and the trade savestates"]
fn two_players_link_up_and_share_a_trade_room() {
    if console(STATE_A, 0).is_none() {
        eprintln!("ROM or savestate missing; nothing to run");
        return;
    }

    let linked = run_from(full_trade, 45, Some("full-trade"), 1160, true).expect("consoles");
    println!(
        "cabled:   {:6} transfers, parent said {:3} distinct, child said {:3} distinct",
        linked.transfers, linked.parent_values, linked.child_values
    );
    println!("  {}", linked.ports);

    let alone = run_from(full_trade, 45, None, 0, false).expect("consoles");
    println!(
        "no cable: {:6} transfers, parent said {:3} distinct, child said {:3} distinct",
        alone.transfers, alone.parent_values, alone.child_values
    );
    println!("  {}", alone.ports);

    // Nothing at all crosses a cable that is not there. If this ever passes
    // with a transfer on it, the cable is being emulated out of thin air.
    assert_eq!(
        alone.transfers, 0,
        "transfers happened with no cable attached"
    );

    // And with one, the two games hold a real conversation. A console that
    // never joined in repeats one value forever; these say dozens of different
    // things to each other, thousands of times.
    assert!(
        linked.transfers > 1_000,
        "only {} transfers crossed the cable",
        linked.transfers
    );
    assert!(
        linked.parent_values > 20 && linked.child_values > 20,
        "one side never really joined in: parent said {} distinct, child said {}",
        linked.parent_values,
        linked.child_values
    );

    // Both end up where a linked pair should be: multi-player mode at 115200,
    // each seeing the other on the far end of the wire.
    for (which, ports) in linked.ports.split(" | ").enumerate() {
        assert!(
            ports.contains("MultiPlayer") && ports.contains("baud=3") && ports.contains("sd=1"),
            "console {which} did not end up linked: {ports}"
        );
    }
}

/// How much network a real trade will put up with.
///
/// The harness links perfectly because it has no network in it. The browser
/// has about a millisecond of round trip after the frame was sliced, and had
/// nearly thirteen before. Sweeping the delay says which of those a game can
/// live with — and whether the browser is now on the right side of the line.
///
/// A healthy run carries tens of thousands of transfers and both consoles say
/// dozens of different things. A link that broke early stops dead, so the
/// numbers falling off a cliff is the answer.
#[test]
#[ignore = "needs a commercial ROM and the trade savestates"]
fn how_much_network_a_trade_survives() {
    if console(STATE_A, 0).is_none() {
        eprintln!("ROM or savestate missing; nothing to run");
        return;
    }

    println!(
        "{:>9}  {:>10}  {:>8}  {:>8}",
        "latency", "transfers", "parent", "child"
    );
    for latency in [0.0, 0.5, 1.0, 2.0, 5.0, 13.0] {
        let outcome = run_with_latency(full_trade, 30, None, 0, true, latency).expect("consoles");
        println!(
            "{:>7.1}ms  {:>10}  {:>8}  {:>8}",
            latency, outcome.transfers, outcome.parent_values, outcome.child_values
        );
    }
}

/// Does one bad round trip in a hundred break a trade?
///
/// The average being fine is not the same as the link being fine. This runs a
/// fast link with the occasional slow transfer, at the sizes actually measured
/// between two browsers.
#[test]
#[ignore = "needs a commercial ROM and the trade savestates"]
fn how_much_jitter_a_trade_survives() {
    if console(STATE_A, 0).is_none() {
        eprintln!("ROM or savestate missing; nothing to run");
        return;
    }

    println!(
        "{:>28}  {:>10}  {:>7}  {:>6}",
        "link", "transfers", "parent", "child"
    );
    for (label, base, spike, every) in [
        ("1ms flat", 1.0, 0.0, 0),
        ("1ms, 17ms every 100", 1.0, 17.0, 100),
        ("1ms, 17ms every 20", 1.0, 17.0, 20),
        ("1ms, 40ms every 100", 1.0, 40.0, 100),
    ] {
        let outcome =
            run_with_jitter(full_trade, 30, None, 0, true, base, spike, every).expect("consoles");
        println!(
            "{label:>28}  {:>10}  {:>7}  {:>6}",
            outcome.transfers, outcome.parent_values, outcome.child_values
        );
    }
}

#[test]
#[ignore = "needs a commercial ROM and the trade savestates"]
fn find_the_input_that_starts_a_trade() {
    if console(STATE_A, 0).is_none() {
        eprintln!("ROM or savestate missing; nothing to run");
        return;
    }

    let strategies: [(&str, Script); 5] = [
        ("both-together", both_together),
        ("child-offset", child_offset),
        ("slow", slow),
        ("walk-up", walk_up_then_talk),
        ("parent-only", parent_only),
    ];

    let mut best: Option<(usize, &str)> = None;
    let mut traded: Vec<&str> = Vec::new();
    for (name, script) in strategies {
        let outcome = run(script, 15, Some(name)).expect("consoles");
        println!(
            "{name:14} transfers {:6}  parent said {:3} distinct  child said {:3} distinct",
            outcome.transfers, outcome.parent_values, outcome.child_values
        );
        println!("    {}", outcome.ports);
        println!("    {}", describe(&outcome.exchange));

        if outcome.exchange.traded() {
            traded.push(name);
        }

        // A console that only ever put one value on the wire never joined in.
        // Two that are actually trading both say many different things.
        let score = outcome.child_values.min(outcome.parent_values);
        if best.is_none_or(|(b, _)| score > b) {
            best = Some((score, name));
        }
    }

    if let Some((score, name)) = best {
        println!("\nfurthest by chatter: {name}, both consoles saying {score} distinct things");
    }

    // The line that matters. Chatter says the cable works; this says the trade
    // finished, and until one of these strategies prints a name here the last
    // step is still unsolved.
    if traded.is_empty() {
        println!("traded: none - every strategy links up and none completes an exchange");
    } else {
        println!("traded: {}", traded.join(", "));
    }
}

/// A run's exchange, in words.
fn describe(exchange: &Exchange) -> String {
    if exchange.traded() {
        return format!(
            "TRADED: parent received {}, child received {}",
            exchange.parent_received, exchange.child_received
        );
    }
    if exchange.parent_received > 0 || exchange.child_received > 0 {
        return format!(
            "half a trade: parent received {}, child received {}",
            exchange.parent_received, exchange.child_received
        );
    }
    if exchange.parties_changed {
        return "no trade, though a party changed somehow".to_string();
    }
    "no trade; both parties identical".to_string()
}
