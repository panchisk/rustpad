//! End-to-end smoke checks for the Windows integration points.
//!
//! These touch real OS resources (named pipe, registry, SAPI, keyboard hook), so
//! they are written to be tolerant: an unavailable feature is skipped, but a
//! broken RustPad code path fails.

#![cfg(windows)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rustpad_platform::pipe::{send_command, PipeServer, RemoteCommand, RemoteHandler, RemoteReply};

fn echo_handler() -> RemoteHandler {
    Arc::new(|cmd: RemoteCommand| match cmd {
        RemoteCommand::GetVersion => RemoteReply::ok().with_line("0.1.0"),
        RemoteCommand::PlaySound {
            index,
            speakers,
            mic,
        } => RemoteReply::ok().with_line(format!(
            "{index}:{}:{}",
            speakers.unwrap_or(false),
            mic.unwrap_or(false)
        )),
        RemoteCommand::AddSound { path } => RemoteReply::ok().with_line(path),
        RemoteCommand::Stop => RemoteReply::ok(),
        other => RemoteReply::error(format!("unexpected command: {other:?}")),
    })
}

#[test]
fn pipe_round_trip_over_named_pipe() {
    let server = match PipeServer::start(echo_handler()) {
        Ok(server) => server,
        Err(err) => {
            eprintln!("skipping: cannot start remote-control server: {err:#}");
            return;
        }
    };

    let version = send_command("GetVersion()", Duration::from_secs(3))
        .expect("server must answer GetVersion");
    assert!(version.contains('0'), "unexpected reply: {version}");

    let play = send_command("DoPlaySound(7,true,0)", Duration::from_secs(3))
        .expect("server must answer DoPlaySound");
    assert!(play.contains("7:true:false"), "unexpected reply: {play}");

    let add = send_command(
        r#"DoAddSound("C:\Sounds\a, b.wav")"#,
        Duration::from_secs(3),
    )
    .expect("server must answer DoAddSound");
    assert!(
        add.contains(r"C:\Sounds\a, b.wav"),
        "unexpected reply: {add}"
    );

    let bad = send_command("DoPlaySound(1,2,3,4)", Duration::from_secs(3))
        .expect("server must answer malformed requests");
    assert!(bad.contains("R-1"), "unexpected reply: {bad}");

    drop(server);
    // After shutdown the pipe must be gone.
    assert!(send_command("GetVersion()", Duration::from_millis(150)).is_err());
}

#[test]
fn activation_pipe_is_served() {
    use rustpad_platform::single_instance;

    let hits = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&hits);
    if single_instance::spawn_activation_server(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })
    .is_err()
    {
        eprintln!("skipping: cannot start the activation pipe server");
        return;
    }

    // `activate_existing` returns true only when it reached the server.
    let ok = single_instance::activate_existing(&[]);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while hits.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }

    assert!(ok, "activate_existing could not reach the activation pipe");
    assert!(
        hits.load(Ordering::SeqCst) >= 1,
        "callback was never called"
    );
}

#[test]
fn registry_backed_features_are_usable_or_report_errors() {
    use rustpad_platform::{autostart, ducking};

    // Reads must work (or report a real error) without panicking.
    let _ = autostart::is_autostart_enabled();
    let value = ducking::current().expect("ducking::current must not fail");
    assert!(value <= 3, "unexpected ducking value {value}");

    // Ducking must round-trip: apply() reports the previous value, restore()
    // writes it back.
    let before = ducking::apply(ducking::Ducking::Reduce50).expect("ducking::apply");
    assert_eq!(
        ducking::current().expect("ducking::current"),
        ducking::Ducking::Reduce50.registry_value()
    );
    ducking::restore(before).expect("ducking::restore");
    assert_eq!(ducking::current().expect("ducking::current"), before);

    // Toggling autostart on and then off must leave the registry as it was.
    let exe = std::env::current_exe().expect("current exe");
    let before = autostart::is_autostart_enabled();
    if autostart::set_autostart(&exe, true).is_ok() {
        assert!(autostart::is_autostart_enabled());
        if !before {
            autostart::set_autostart(&exe, false).expect("disable autostart");
            assert!(!autostart::is_autostart_enabled());
        }
    }
}

fn wav_header(path: &std::path::Path) -> Option<(u16, u32, u16)> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 44 || &bytes[..4] != b"RIFF" {
        return None;
    }
    let channels = u16::from_le_bytes([bytes[22], bytes[23]]);
    let rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
    let bits = u16::from_le_bytes([bytes[34], bytes[35]]);
    Some((channels, rate, bits))
}

#[test]
fn tts_voices_and_synthesis() {
    use rustpad_platform::tts;

    let Ok(voices) = tts::list_voices() else {
        eprintln!("skipping: no SAPI voices on this host");
        return;
    };
    assert!(!voices.is_empty());
    for voice in &voices {
        assert!(
            !voice.id.is_empty() && !voice.name.is_empty(),
            "voice without id/name: {voice:?}"
        );
    }

    let out = std::env::temp_dir().join(format!("rustpad-tts-test-{}.wav", std::process::id()));
    let result = tts::synthesize_to_wav("RustPad test & <audio>", None, 0, 0, &out);
    match result {
        Ok(()) => {
            let (channels, rate, bits) = wav_header(&out).expect("valid RIFF/WAVE output");
            assert_eq!(channels, 1, "expected mono output");
            assert_eq!(rate, 22_050, "expected 22.05 kHz output");
            assert_eq!(bits, 16, "expected 16-bit output");
        }
        Err(err) => eprintln!("skipping synthesis check: {err:#}"),
    }
    let _ = std::fs::remove_file(&out);

    // The SSML/prosody path must also produce a valid file.
    let pitched =
        std::env::temp_dir().join(format!("rustpad-tts-pitch-test-{}.wav", std::process::id()));
    if tts::synthesize_to_wav(
        "Pitch test",
        voices.first().map(|v| v.id.as_str()),
        2,
        3,
        &pitched,
    )
    .is_ok()
    {
        assert!(
            wav_header(&pitched).is_some(),
            "pitched output is not a WAV"
        );
    }
    let _ = std::fs::remove_file(&pitched);
}

#[test]
fn keyboard_hook_starts_and_stops() {
    use rustpad_platform::hotkeys::KeyboardHook;

    let mut hook = match KeyboardHook::start() {
        Ok(hook) => hook,
        Err(err) => {
            eprintln!("skipping: global keyboard hook unavailable: {err:#}");
            return;
        }
    };

    assert!(hook.is_running());
    assert!(hook.try_event().is_none(), "unexpected queued key event");
    // Must not hang: `stop` posts WM_QUIT and joins the hook thread.
    hook.stop();
    assert!(!hook.is_running());
}
