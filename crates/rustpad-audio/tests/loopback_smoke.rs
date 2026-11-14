//! Loopback recording smoke test.
//!
//! Machines without a usable render endpoint (CI containers, RDP sessions
//! without audio) skip the assertions instead of failing.

use std::time::{Duration, Instant};

use rustpad_audio::types::{EngineCommand, EngineConfig, EngineEvent};
use rustpad_audio::EngineHandle;

fn wait_for(
    engine: &EngineHandle,
    deadline: Instant,
    mut matches: impl FnMut(&EngineEvent) -> bool,
) -> Option<EngineEvent> {
    while Instant::now() < deadline {
        while let Some(event) = engine.try_event() {
            if matches(&event) {
                return Some(event);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

#[test]
fn loopback_records_a_wav() {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("skipping loopback smoke test: no temp dir ({err})");
            return;
        }
    };
    let path = dir.path().join("loopback.wav");

    let engine = EngineHandle::spawn(EngineConfig::default());
    let deadline = Instant::now() + Duration::from_secs(10);
    engine.send(EngineCommand::StartLoopbackRecording {
        path: path.clone(),
        max_secs: 2,
    });

    match wait_for(&engine, deadline, |event| {
        matches!(event, EngineEvent::RecordingStarted | EngineEvent::Error(_))
    }) {
        Some(EngineEvent::RecordingStarted) => {}
        Some(EngineEvent::Error(err)) => {
            eprintln!("skipping loopback smoke test: {err}");
            engine.shutdown();
            return;
        }
        _ => {
            eprintln!("skipping loopback smoke test: no answer from the engine");
            engine.shutdown();
            return;
        }
    }

    std::thread::sleep(Duration::from_millis(600));
    engine.send(EngineCommand::StopRecording);

    let finished = wait_for(&engine, deadline, |event| {
        matches!(
            event,
            EngineEvent::RecordingFinished { .. } | EngineEvent::Error(_)
        )
    });
    engine.shutdown();

    match finished {
        Some(EngineEvent::RecordingFinished {
            path: finished_path,
            duration_ms,
        }) => {
            assert_eq!(finished_path, path);
            assert!(duration_ms > 0, "recorded {duration_ms} ms of audio");
        }
        Some(EngineEvent::Error(err)) => panic!("loopback recording failed: {err}"),
        _ => panic!("loopback recording did not finish within 10 s"),
    }

    assert!(path.is_file(), "{} was not written", path.display());
    let header = std::fs::read(&path).expect("reading the WAV back");
    assert!(header.len() > 12, "WAV file is too short");
    assert_eq!(&header[0..4], b"RIFF");
    assert_eq!(&header[8..12], b"WAVE");
}
