//! RustPad: a Rust soundboard for Windows with Soundpad-compatible `.spl` lists
//! and remote control.

mod app;
mod theme;
mod ui;

use std::path::PathBuf;

use anyhow::Result;
use rustpad_core::config::{config_path, AppConfig};
use rustpad_platform::single_instance;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rustpad=info,rustpad_audio=warn".into()),
        )
        .with_target(false)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--help" || a == "-h" || a == "/?") {
        print_help();
        return Ok(());
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!(
            "RustPad {} (Soundpad 4.x compatible)",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--list-devices") {
        let (out, inp) = rustpad_audio::EngineHandle::devices();
        println!("Output devices:");
        for d in out {
            println!(
                "  {}{}",
                d.name,
                if d.is_default { " [default]" } else { "" }
            );
        }
        println!("Input devices:");
        for d in inp {
            println!(
                "  {}{}",
                d.name,
                if d.is_default { " [default]" } else { "" }
            );
        }
        return Ok(());
    }
    if let Some(pos) = args.iter().position(|a| a == "--selftest-rewrite") {
        let (Some(input), Some(output)) = (args.get(pos + 1), args.get(pos + 2)) else {
            eprintln!("usage: rustpad --selftest-rewrite <in.spl> <out.spl>");
            std::process::exit(2);
        };
        let list = rustpad_core::Soundlist::load(std::path::Path::new(input))?;
        list.save(std::path::Path::new(output))?;
        println!(
            "rewrote {} -> {}: {} sounds, {} categories, {} hotbar pages",
            input,
            output,
            list.sounds.len(),
            list.categories.len(),
            list.hotbar.len()
        );
        return Ok(());
    }
    if let Some(pos) = args.iter().position(|a| a == "--selftest-decode") {
        let Some(path) = args.get(pos + 1) else {
            eprintln!("usage: rustpad --selftest-decode <file>");
            std::process::exit(2);
        };
        let audio = rustpad_audio::decode_file(std::path::Path::new(path))?;
        println!(
            "{}: {} ms, {} Hz, {} ch, peak {:.1} dBFS, RMS {:.1} dBFS",
            path,
            audio.duration_ms(),
            audio.sample_rate,
            audio.channels,
            rustpad_audio::dsp::peak_db(&audio.samples),
            rustpad_audio::dsp::rms_db(&audio.samples)
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--selftest-loopback") {
        let path = std::env::temp_dir().join("rustpad-selftest-loopback.wav");
        let handle =
            rustpad_audio::EngineHandle::spawn(rustpad_audio::types::EngineConfig::default());
        handle.send(rustpad_audio::EngineCommand::StartLoopbackRecording {
            path: path.clone(),
            max_secs: 1,
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        let mut finished = false;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(100));
            while let Some(ev) = handle.try_event() {
                println!("event: {ev:?}");
                if matches!(ev, rustpad_audio::EngineEvent::RecordingFinished { .. }) {
                    finished = true;
                }
            }
            if finished {
                break;
            }
        }
        handle.shutdown();
        if !finished {
            anyhow::bail!("loopback recording did not finish");
        }
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        println!("loopback: {size} bytes at {}", path.display());
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    if args.iter().any(|a| a == "--selftest-mic") {
        let secs: u64 = args
            .iter()
            .position(|a| a == "--selftest-mic")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(3);
        return selftest_mic(secs);
    }
    if args.iter().any(|a| a == "--selftest-audio") {
        return selftest_audio();
    }
    if args.iter().any(|a| a == "--selftest-list") {
        let path = args
            .iter()
            .position(|a| a == "--selftest-list")
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../docs/evidence/sample-soundlist.spl"
                ))
            });
        let list = rustpad_core::Soundlist::load(&path)?;
        println!(
            "loaded {}: {} sounds, {} categories, {} hotbar pages",
            path.display(),
            list.sounds.len(),
            list.categories.len(),
            list.hotbar.len()
        );
        for c in &list.categories {
            println!(
                "  category {:?} icon={} sounds={}",
                c.name,
                c.icon,
                c.sound_ids.len()
            );
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--remote-control" || a == "-rc") {
        let command = args
            .iter()
            .skip_while(|a| *a != "-rc" && *a != "--remote-control")
            .nth(1)
            .cloned()
            .unwrap_or_default();
        if command.is_empty() {
            eprintln!("usage: rustpad -rc \"DoPlaySound(1)\"");
            std::process::exit(2);
        }
        match rustpad_platform::pipe::send_command(&command, std::time::Duration::from_secs(3)) {
            Ok(reply) => {
                print!("{reply}");
                Ok(())
            }
            Err(err) => {
                eprintln!("rustpad: {err:#}");
                std::process::exit(1);
            }
        }
    } else if args.iter().any(|a| a == "--import") {
        // Headless import: append files to the configured list and exit.
        let paths: Vec<PathBuf> = args
            .iter()
            .skip_while(|a| *a != "--import")
            .skip(1)
            .filter(|a| !a.starts_with('-'))
            .map(PathBuf::from)
            .collect();
        headless_import(paths)
    } else {
        run_gui(args)
    }
}

fn selftest_mic(secs: u64) -> Result<()> {
    use std::time::Duration;

    let path = std::env::temp_dir().join("rustpad-selftest-mic.wav");
    let handle = rustpad_audio::EngineHandle::spawn(rustpad_audio::types::EngineConfig::default());
    handle.send(rustpad_audio::EngineCommand::StartRecording {
        path: path.clone(),
        max_secs: secs.min(30) as u32,
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(secs + 2);
    let mut finished = false;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        while let Some(ev) = handle.try_event() {
            if matches!(ev, rustpad_audio::EngineEvent::RecordingFinished { .. }) {
                finished = true;
            }
        }
        if finished {
            break;
        }
    }
    handle.send(rustpad_audio::EngineCommand::StopRecording);
    std::thread::sleep(Duration::from_millis(300));
    handle.shutdown();
    let audio = rustpad_audio::decode_file(&path)?;
    println!(
        "mic: {} ms, peak {:.1} dBFS, RMS {:.1} dBFS",
        audio.duration_ms(),
        rustpad_audio::dsp::peak_db(&audio.samples),
        rustpad_audio::dsp::rms_db(&audio.samples)
    );
    let _ = std::fs::remove_file(&path);
    Ok(())
}

fn selftest_audio() -> Result<()> {
    use std::sync::Arc;
    use std::time::Duration;

    let (out, inp) = rustpad_audio::EngineHandle::devices();
    println!("outputs: {}", out.len());
    for d in &out {
        println!(
            "  {}{}",
            d.name,
            if d.is_default { " [default]" } else { "" }
        );
    }
    println!("inputs: {}", inp.len());
    for d in &inp {
        println!(
            "  {}{}",
            d.name,
            if d.is_default { " [default]" } else { "" }
        );
    }

    let sr = 48_000u32;
    let frames = sr as usize * 12 / 10;
    let mut samples = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        let v = (n as f32 / sr as f32 * 440.0 * std::f32::consts::TAU).sin() * 0.25;
        samples.push(v);
        samples.push(v);
    }
    let tmp = std::env::temp_dir().join("rustpad-selftest.wav");
    rustpad_audio::wav::write_wav(&tmp, sr, 2, &samples)?;
    let decoded = rustpad_audio::decode_file(&tmp)?;
    println!(
        "wrote + decoded {} ({} ms, {} Hz, {} ch)",
        tmp.display(),
        decoded.duration_ms(),
        decoded.sample_rate,
        decoded.channels
    );

    let handle = rustpad_audio::EngineHandle::spawn(rustpad_audio::types::EngineConfig::default());
    let audio = Arc::new(decoded);
    let request = rustpad_audio::types::PlayRequest {
        sound_id: 1,
        source: rustpad_audio::types::PlaySource::Decoded(audio.clone()),
        gain: 1.0,
        targets: vec![rustpad_audio::types::Target::Speakers],
        looping: false,
        start_ms: 0,
    };
    handle.send(rustpad_audio::EngineCommand::PlayDecoded { request, audio });

    let rec_path = std::env::temp_dir().join("rustpad-selftest-rec.wav");
    handle.send(rustpad_audio::EngineCommand::StartRecording {
        path: rec_path.clone(),
        max_secs: 2,
    });

    let deadline = std::time::Instant::now() + Duration::from_millis(2500);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        while let Some(ev) = handle.try_event() {
            println!("event: {ev:?}");
        }
        let s = handle.status();
        if s.voices > 0 {
            println!(
                "playing sound {:?} at {} / {} ms, out {:.3} mic {:.3}",
                s.sound_id,
                s.position_ms,
                s.duration_ms,
                handle.output_level(),
                handle.mic_level()
            );
        }
    }
    handle.send(rustpad_audio::EngineCommand::StopRecording);
    std::thread::sleep(Duration::from_millis(500));
    while let Some(ev) = handle.try_event() {
        println!("event: {ev:?}");
    }
    if rec_path.exists() {
        println!(
            "recording: {} bytes at {}",
            std::fs::metadata(&rec_path).map(|m| m.len()).unwrap_or(0),
            rec_path.display()
        );
    } else {
        println!("recording: no file (no capture device or no audio)");
    }
    println!(
        "mic level: {:.3}, output level: {:.3}",
        handle.mic_level(),
        handle.output_level()
    );
    handle.shutdown();
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_file(&rec_path);
    Ok(())
}

fn headless_import(paths: Vec<PathBuf>) -> Result<()> {
    let cfg_path = config_path();
    let cfg = AppConfig::load_or_default(&cfg_path);
    let mut list = rustpad_core::Soundlist::load(&cfg.soundlist_path)
        .unwrap_or_else(|_| rustpad_core::Soundlist::new());
    for path in paths {
        match rustpad_audio::decode::probe_duration_ms(&path) {
            Ok(ms) => {
                let id = list.add_path(path.clone(), ms);
                if let Some(s) = list.find_mut(id) {
                    s.hash = app::hash_for_path(&path);
                }
                println!("added: {}", path.display());
            }
            Err(err) => eprintln!("skipping {}: {err:#}", path.display()),
        }
    }
    if cfg.soundlist_path.exists() {
        let backup = cfg.soundlist_path.with_file_name(format!(
            "backup-soundlist-{}.spl",
            rustpad_core::today_iso()
        ));
        if !backup.exists() {
            let _ = std::fs::copy(&cfg.soundlist_path, &backup);
        }
    }
    list.save(&cfg.soundlist_path)?;
    Ok(())
}

fn run_gui(args: Vec<String>) -> Result<()> {
    let initial_list = args
        .iter()
        .position(|a| a == "--list")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    // Single instance: forward activation to the running window.
    let guard = match single_instance::acquire() {
        Ok(Some(g)) => Some(g),
        Ok(None) => {
            let _ = single_instance::activate_existing(&[]);
            return Ok(());
        }
        Err(err) => {
            tracing::warn!("single-instance guard unavailable: {err:#}");
            None
        }
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([840.0, 520.0])
            .with_title("RustPad")
            .with_app_id("rustpad"),
        ..Default::default()
    };

    let result = eframe::run_native(
        "RustPad",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(match initial_list {
                Some(path) => app::AppState::new_with_list(cc, guard, Some(path)),
                None => app::AppState::new(cc, guard),
            }))
        }),
    );
    result.map_err(|e| anyhow::anyhow!("eframe: {e}"))
}

fn print_help() {
    println!(
        "RustPad: Rust soundboard for Windows (Soundpad 4.x compatible)

usage:
  rustpad [--list <f.spl>]    start the GUI (optionally with another sound list)
  rustpad -rc \"<command>\"      send a remote-control command to the running app
  rustpad --import <files...>  append files to the configured sound list and exit
  rustpad --list-devices       list WASAPI input/output devices
  rustpad --selftest-audio     play a test tone and record 2.5 s from the default mic
  rustpad --selftest-loopback  record 1 s of system output (WASAPI loopback)
  rustpad --selftest-list [f]  parse a .spl file and print its contents
  rustpad --selftest-decode <f> decode an audio file and print its format/levels
  rustpad --selftest-rewrite <in> <out>  load a .spl and write it back
  rustpad --version | --help

remote-control commands (Soundpad-compatible):
  DoPlaySound(index)
  DoPlaySound(index,speakers,mic)
  DoPlaySoundFromCategory(categoryIndex,soundIndex,speakers,mic)
  DoPlayRandomSound(speakers,mic)
  DoPlayRandomSoundFromCategory(categoryIndex,speakers,mic)
  DoPlayPreviousSound() / DoPlayNextSound() / DoPlaySelectedSound()
  DoPlayCurrentSoundAgain() / DoPlayPreviouslyPlayedSound()
  DoAddSound(path) / DoRemoveSelectedEntries() / DoRemoveCategory() / DoSaveSoundlist()
  GetSoundFileCount() / GetSoundlist() / GetVersion() / GetRemoteControlVersion()
  PauseResume() / Stop()
"
    );
}
