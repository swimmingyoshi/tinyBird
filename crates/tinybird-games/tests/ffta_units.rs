//! Read the tutorial battle's units out of a real cartridge.
//!
//! Ignored by default: it needs the commercial ROM and a local savestate,
//! neither of which is in the repository. The unit tests in `ffta::units` cover
//! the same logic against the bytes copied out of this state, so CI still
//! catches a broken decode. This exists because those bytes were copied by
//! hand, and a transcription that is wrong in the same way twice is exactly
//! what a fixture cannot catch.
//!
//! Run it with:
//!
//! ```text
//! cargo test -p tinybird-games --test ffta_units -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};

use tinybird_addons::{GameAddon, RomIdentity};
use tinybird_core::gba::Gba;
use tinybird_games::ffta::units::{
    find_roster_except, read_units, read_units_at, CLAN_VITALS_BASE,
};
use tinybird_games::ffta::FftaAddon;
use tinybird_games::{AddonData, GbaMemory};

fn workspace(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

#[test]
#[ignore = "Needs a commercial ROM and a local savestate"]
fn the_tutorial_battle_units_are_named_from_the_cartridge() {
    let rom_path =
        workspace("roms/Final Fantasy Tactics Advance/Final Fantasy Tactics Advance (USA).gba");
    let state_path =
        workspace("roms/Final Fantasy Tactics Advance/Final Fantasy Tactics Advance (USA).state");
    if !rom_path.is_file() || !state_path.is_file() {
        eprintln!("need both an FFTA ROM and a savestate; nothing to read");
        return;
    }

    let mut gba = Gba::new();
    gba.load_rom(std::fs::read(&rom_path).expect("read FFTA ROM"));
    gba.load_state_bytes(&std::fs::read(&state_path).expect("read savestate"))
        .expect("deserialize FFTA savestate");

    let units = read_units(&GbaMemory(&gba));
    for unit in &units {
        println!(
            "slot {}  {:<10}  HP {:>7}  MP {}",
            unit.slot,
            unit.display_name(),
            unit.hp_text(),
            unit.mp_text()
        );
    }

    // The whole snowball fight, both sides. The old hard-coded address was
    // slot 6 of this roster, which is why the panel used to show the last
    // three of these and nothing else.
    let names: Vec<_> = units.iter().map(|unit| unit.display_name()).collect();
    assert_eq!(
        names,
        [
            "Lyle",
            "Guinness",
            "Colin",
            "Tia",
            "MarcheAAA",
            "Mewt",
            "Ritz",
            "Norma",
            "Leslaie"
        ]
    );

    // Ritz's panel read HP 16/16 MP 10/10 on screen, which is where every
    // address in this module started.
    let ritz = units
        .iter()
        .find(|unit| unit.display_name() == "Ritz")
        .unwrap();
    assert_eq!(ritz.hp_text(), "16/16");
    assert_eq!(ritz.mp_text(), "10/10");
}

#[test]
#[ignore = "Needs a commercial ROM and a local savestate"]
fn a_real_battle_roster_is_found_and_the_party_told_apart() {
    let rom_path =
        workspace("roms/Final Fantasy Tactics Advance/Final Fantasy Tactics Advance (USA).gba");
    let state_path = workspace("Final_Fantasy_Tactics_Advance_Battle_.state");
    if !rom_path.is_file() || !state_path.is_file() {
        eprintln!("need both an FFTA ROM and the battle savestate; nothing to read");
        return;
    }

    let mut gba = Gba::new();
    gba.load_rom(std::fs::read(&rom_path).expect("read FFTA ROM"));
    gba.load_state_bytes(&std::fs::read(&state_path).expect("read savestate"))
        .expect("deserialize FFTA savestate");

    let units = read_units(&GbaMemory(&gba));
    for unit in &units {
        println!(
            "{} {:<10} HP {:>7} MP {}",
            if unit.player { "yours " } else { "      " },
            unit.display_name(),
            unit.hp_text(),
            unit.mp_text()
        );
    }

    // The address the addon used to hard-code holds an unrelated record in this
    // state, and reading it reported one unit called "Judge". The roster is
    // found now, so the whole field is here.
    assert!(units.len() >= 12, "found only {} units", units.len());

    let mine: Vec<_> = units
        .iter()
        .filter(|unit| unit.player)
        .map(|unit| unit.display_name())
        .collect();
    // FFTA fields at most six, and the on-screen party is exactly these.
    assert_eq!(
        mine,
        [
            "Udvil",
            "Smyth",
            "Gallahan",
            "Javet",
            "Montblanc",
            "MarcheAAA"
        ]
    );

    // The player's own name was typed in, so their record points into work RAM
    // rather than the cartridge. Reading it is what proves both windows work.
    assert!(units.iter().any(|unit| unit.display_name() == "MarcheAAA"));

    let clan = read_units_at(&GbaMemory(&gba), CLAN_VITALS_BASE);
    for unit in &clan {
        println!(
            "{} {:<10} {:<7} {}",
            if unit.player { "set" } else { "   " },
            unit.display_name(),
            unit.race_name().unwrap_or("?"),
            unit.job_name().unwrap_or("?")
        );
    }
    assert!(
        clan.len() > 6,
        "the persistent clan list should include reserves"
    );
    assert!(clan.iter().any(|unit| unit.race_name() == Some("Moogle")));

    let rom = RomIdentity {
        title: "FFTA_USVER.".to_string(),
        game_code: "AFXE".to_string(),
        maker_code: "01".to_string(),
        revision: 0,
        fingerprint: 0,
    };
    let addon = FftaAddon::default()
        .snapshot(&GbaMemory(&gba), &rom)
        .expect("FFTA addon snapshot");
    let AddonData::Ffta(snapshot) = addon.data else {
        panic!("expected FFTA data");
    };
    assert!(snapshot.in_battle);
    assert_eq!(
        snapshot.units.len(),
        clan.iter().filter(|unit| unit.player).count()
    );
    assert!(snapshot
        .units
        .iter()
        .all(|unit| unit.display_name() != "Judge"));
    assert!(
        !snapshot.enemies.is_empty(),
        "battle should expose opponents"
    );
    assert!(snapshot.enemies.iter().all(|unit| !unit.is_judge()));
}

#[test]
#[ignore = "Needs the user-provided FFTA team-setup savestate"]
fn the_real_team_setup_state_previews_its_enemy_team() {
    let rom_path =
        workspace("roms/Final Fantasy Tactics Advance/Final Fantasy Tactics Advance (USA).gba");
    let state_path =
        workspace("tb_bd3d3e5129e94331a95d91466eb69a90_AFXE_1788122234507_FinalFantasyTa.state");
    if !rom_path.is_file() || !state_path.is_file() {
        eprintln!("need both the FFTA ROM and team-setup savestate; nothing to read");
        return;
    }

    let mut gba = Gba::new();
    gba.load_rom(std::fs::read(&rom_path).expect("read FFTA ROM"));
    gba.load_state_bytes(&std::fs::read(&state_path).expect("read setup savestate"))
        .expect("deserialize setup savestate");
    // The web save is captured before the first composed frame. Advancing once
    // matches what the browser displays and lets any pending memory writes land.
    gba.run_frame();

    let memory = GbaMemory(&gba);
    if let Some(base) = find_roster_except(&memory, Some(CLAN_VITALS_BASE)) {
        println!("alternate roster at {base:#010x}");
        for unit in read_units_at(&memory, base) {
            println!(
                "field {} {:<10} {:<10} {}",
                if unit.player { "yours" } else { "other" },
                unit.display_name(),
                unit.race_name().unwrap_or("?"),
                unit.job_name().unwrap_or("?")
            );
        }
    } else {
        println!("no alternate roster found");
    }

    let rom = RomIdentity {
        title: "FFTA_USVER.".to_string(),
        game_code: "AFXE".to_string(),
        maker_code: "01".to_string(),
        revision: 0,
        fingerprint: 0,
    };
    let addon = FftaAddon::default()
        .snapshot(&memory, &rom)
        .expect("FFTA setup snapshot");
    let AddonData::Ffta(snapshot) = addon.data else {
        panic!("expected FFTA data");
    };

    println!("clan/party: {}", snapshot.units.len());
    for unit in &snapshot.enemies {
        println!(
            "enemy {:<10} {:<10} {}",
            unit.display_name(),
            unit.race_name().unwrap_or("?"),
            unit.job_name().unwrap_or("?")
        );
    }
    assert!(!snapshot.in_battle, "this state is still on team setup");
    assert!(
        !snapshot.enemies.is_empty(),
        "team setup should expose the prepared enemy roster"
    );
    assert!(snapshot.enemies.iter().all(|unit| !unit.is_judge()));
    assert_eq!(
        addon.sections[1].note.as_deref(),
        Some("Setup preview · 4 opponents")
    );
}

#[test]
#[ignore = "Needs a commercial ROM and a local savestate"]
fn finding_the_roster_costs_a_fraction_of_a_frame() {
    // The scan walks work RAM, and this addon runs once per frame in a browser
    // with an 11.8ms frame against a 16.74ms budget. A number here is worth
    // more than an opinion about it.
    let rom_path =
        workspace("roms/Final Fantasy Tactics Advance/Final Fantasy Tactics Advance (USA).gba");
    let state_path = workspace("Final_Fantasy_Tactics_Advance_Battle_.state");
    if !rom_path.is_file() || !state_path.is_file() {
        return;
    }

    let mut gba = Gba::new();
    gba.load_rom(std::fs::read(&rom_path).expect("read FFTA ROM"));
    gba.load_state_bytes(&std::fs::read(&state_path).expect("read savestate"))
        .expect("deserialize FFTA savestate");

    let memory = GbaMemory(&gba);
    let start = std::time::Instant::now();
    let runs = 60;
    for _ in 0..runs {
        std::hint::black_box(read_units(&memory));
    }
    let each = start.elapsed() / runs;
    println!("find + read: {each:?} per call");

    // Native, release, so not the wasm number — but a millisecond here would
    // mean trouble there, and this is the check that would catch it.
    assert!(
        each < std::time::Duration::from_millis(4),
        "roster scan took {each:?}, which is too much of a frame"
    );
}
