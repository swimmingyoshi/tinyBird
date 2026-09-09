//! Local JSON-lines transport: one isolated runtime per child process.
use std::io::{self, BufRead, Write};
use tinybird_core::Gba;
use tinybird_runtime::{Command, ObservationConfig, Runtime};

fn main() {
    if let Err(error) = run() {
        eprintln!("tinybird-headless: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let rom = args
        .next()
        .ok_or("usage: tinybird-headless ROM [--bios PATH] [--state PATH] [--observations PATH]")?;
    if rom == "--help" {
        println!("tinybird-headless ROM [--bios PATH] [--state PATH] [--observations PATH]\nReads JSON commands from stdin; writes JSON replies to stdout.");
        return Ok(());
    }
    let mut gba = Gba::with_rom(std::fs::read(rom)?);
    let mut state = None;
    let mut observations = None;
    while let Some(flag) = args.next() {
        let path = args.next().ok_or("option requires a path")?;
        match flag.as_str() {
            "--bios" => gba.load_bios(std::fs::read(path)?),
            "--state" => state = Some(std::fs::read(path)?),
            "--observations" => {
                let bytes = std::fs::read(path)?;
                if bytes.len() > 1_048_576 {
                    return Err("observation config exceeds 1 MiB".into());
                }
                observations = Some(serde_json::from_slice::<ObservationConfig>(&bytes)?);
            }
            _ => return Err(format!("unknown option: {flag}").into()),
        }
    }
    gba.start();
    // No host-clock updates during stepping: repeatable cartridge time.
    gba.set_wall_clock(946684800);
    if let Some(state) = state {
        gba.load_state_bytes(&state)?;
    }
    let mut runtime = Runtime::new(gba)?;
    if let Some(config) = observations {
        runtime.execute(Command::ConfigureObservations { config })?;
    }
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    loop {
        let mut line = String::new();
        let count = std::io::Read::take(&mut input, 1_048_577).read_line(&mut line)?;
        if count == 0 {
            break;
        }
        if count > 1_048_576 {
            return Err("command exceeds 1 MiB".into());
        }
        let result = serde_json::from_str::<Command>(&line)
            .map_err(|e| e.to_string())
            .and_then(|command| runtime.execute(command));
        let reply = match result {
            Ok(value) => serde_json::json!({"ok": true, "result": value}),
            Err(error) => serde_json::json!({"ok": false, "error": error}),
        };
        serde_json::to_writer(&mut output, &reply)?;
        writeln!(output)?;
        output.flush()?;
    }
    Ok(())
}
