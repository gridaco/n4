use anyhow::Result;
use n4_engine::{
    FrameRate, Project,
    export::{AudioClip, sample_boundary, write_pcm},
};
use std::{fs, io::Write, path::Path};

#[test]
fn fractional_frame_boundaries_do_not_accumulate_rounding_error() -> Result<()> {
    let rate = FrameRate(30_000, 1001);
    let ends = (0..=5)
        .map(|i| sample_boundary(rate, i))
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(ends, [0, 1602, 3203, 4805, 6406, 8008]);
    assert_eq!(sample_boundary(rate, 30_000)?, 48_048_000);
    Ok(())
}

/// Hand-build a stereo PCM16 WAV. Our oracle never asks FFmpeg where to trim or mix.
fn source(path: &Path) -> Result<()> {
    let mut out = fs::File::create_new(path)?;
    let bytes = 16u32;
    out.write_all(b"RIFF")?;
    out.write_all(&(36 + bytes).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&16u32.to_le_bytes())?;
    for value in [1u16, 2] {
        out.write_all(&value.to_le_bytes())?;
    }
    for value in [48_000u32, 192_000] {
        out.write_all(&value.to_le_bytes())?;
    }
    for value in [4u16, 16] {
        out.write_all(&value.to_le_bytes())?;
    }
    out.write_all(b"data")?;
    out.write_all(&bytes.to_le_bytes())?;
    for value in [0i16, 0, 8192, -8192, 16384, -16384, 24576, -24576] {
        out.write_all(&value.to_le_bytes())?;
    }
    Ok(())
}

#[test]
fn mix_overlaps_at_exact_samples_across_blocks_and_rejects_source_overrun() -> Result<()> {
    let mut edit =
        Project::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/title.json"))?;
    let tmp = tempfile::tempdir()?;
    let wave = tmp.path().join("timed audio.wav");
    source(&wave)?;
    edit.audio = vec![
        AudioClip {
            source: wave.clone(),
            start_sample: 8191,
            source_in_sample: 1,
            samples: 3,
            gain: 0.5,
        },
        AudioClip {
            source: wave,
            start_sample: 8192,
            source_in_sample: 3,
            samples: 1,
            gain: 1.0,
        },
    ];
    let pcm = tmp.path().join("mix.f32le");
    write_pcm(&edit, &pcm)?;
    let bytes = fs::read(&pcm)?;
    assert_eq!(bytes.len(), 96_000 * 8);
    for (n, frame) in bytes.as_chunks::<8>().0.iter().enumerate() {
        let expected: f32 = match n {
            8191 => 0.125,
            8192 => 1.0,
            8193 => 0.375,
            _ => 0.0,
        };
        let l = f32::from_le_bytes(frame[..4].try_into()?);
        let r = f32::from_le_bytes(frame[4..].try_into()?);
        assert_eq!([l, r], [expected, -expected]);
    }
    assert!(write_pcm(&edit, &pcm).is_err());
    assert_eq!(fs::read(&pcm)?, bytes);
    edit.audio[0].samples += 1;
    let missing = tmp.path().join("rejected.f32le");
    assert!(write_pcm(&edit, &missing).is_err());
    assert!(!missing.exists());
    Ok(())
}

#[test]
fn session_rejects_broken_audio_and_checks_exact_resampled_bounds_before_accepting() -> Result<()> {
    use n4_engine::session::Session;
    use serde_json::json;
    use std::process::Command;

    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let tmp = tempfile::tempdir()?;
    let audio = tmp.path().join("resampled.aac.m4a");
    let encoded = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=500:sample_rate=44100:duration=0.115",
            "-c:a",
            "aac",
        ])
        .arg(&audio)
        .output()?;
    assert!(
        encoded.status.success(),
        "{}",
        String::from_utf8_lossy(&encoded.stderr)
    );
    // Count the actual samples of the decoded 44.1 kHz source on the 48 kHz bus.
    // Codec priming and resampling make stream duration an insufficient oracle.
    let decoded = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&audio)
        .args([
            "-map", "0:a:0", "-ar", "48000", "-ac", "2", "-f", "f32le", "pipe:1",
        ])
        .output()?;
    assert!(decoded.status.success() && !decoded.stdout.is_empty());
    let samples = (decoded.stdout.len() / 8) as u64;
    assert_eq!(decoded.stdout.len() % 8, 0);

    let mut edit = Project::load(&fixtures.join("title.json"))?;
    edit.audio.push(AudioClip {
        source: audio,
        start_sample: 0,
        source_in_sample: 0,
        samples,
        gain: 1.0,
    });
    let project_path = tmp.path().join("session.json");
    fs::write(&project_path, serde_json::to_vec(&edit)?)?;
    let mut session = Session::open(&project_path)?;
    assert_eq!(session.revision(), 0);
    assert!(
        session
            .set(0, "/audio/0/samples", json!(samples + 1))
            .is_err()
    );
    assert_eq!(session.revision(), 0);
    let bad = tmp.path().join("malformed.aac");
    fs::write(&bad, b"this is not an audio stream")?;
    assert!(session.set(0, "/audio/0/source", json!(bad)).is_err());
    assert_eq!(session.revision(), 0);
    session.set(0, "/audio/0/samples", json!(samples - 1))?;
    assert_eq!(session.revision(), 1);
    assert_eq!(session.render(29)?.trace.request.source_frame, Some(39));
    Ok(())
}
