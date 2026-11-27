//! On-device measuring instrument. The engine produces all pixels and the offline mix.
use anyhow::{Context, Result, anyhow, ensure};
use clap::Parser;
use n4_engine::{
    Project, export,
    preview::{Playback, Preview},
};
use sdl2::{
    audio::{AudioCallback, AudioSpecDesired},
    event::Event,
    pixels::PixelFormatEnum,
};
use serde_json::json;
use std::{
    fs,
    io::{BufReader, Read},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Parser)]
struct Args {
    project: PathBuf,
    #[arg(long, default_value_t = 10)]
    seconds: u32,
    /// Schedule the entire cut, including a final fractional second.
    #[arg(long, conflicts_with = "seconds")]
    whole_project: bool,
    /// Measure fresh-decoder and repeated seeks before starting playback.
    #[arg(long, default_value_t = 0)]
    seek_count: u32,
    #[arg(long)]
    report: Option<PathBuf>,
}

// One block holds 1,024 sample frames of stereo PCM. Prebuffer ~85 ms before play.
struct Block {
    start: u64,
    data: Vec<f32>,
}
struct Audio {
    blocks: mpsc::Receiver<Block>,
    current: Option<Block>,
    cursor: u64,
    total: u64,
    submitted: Arc<AtomicU64>,
    missing: Arc<AtomicU64>,
}
impl AudioCallback for Audio {
    type Channel = f32;
    fn callback(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let mut pos = 0;
        while pos < out.len() && self.cursor < self.total {
            if self.current.is_none() {
                self.current = self.blocks.try_recv().ok();
            }
            let Some(block) = self.current.as_ref() else {
                let samples = ((out.len() - pos) as u64 / 2).min(self.total - self.cursor);
                self.missing.fetch_add(samples, Ordering::Relaxed);
                self.cursor += samples;
                break;
            };
            let offset = ((self.cursor - block.start) * 2) as usize;
            if offset >= block.data.len() {
                self.current = None;
                continue;
            }
            let n = (out.len() - pos).min(block.data.len() - offset);
            for (dst, src) in out[pos..pos + n]
                .iter_mut()
                .zip(&block.data[offset..offset + n])
            {
                *dst = *src * 0.10; // Quiet monitoring gain. Exported samples are unmodified.
            }
            pos += n;
            self.cursor += n as u64 / 2;
        }
        // Audio demand continues through the last callback after the exact project end.
        self.submitted
            .fetch_add(out.len() as u64 / 2, Ordering::Release);
    }
}

fn pump(path: PathBuf, send: mpsc::SyncSender<Block>, primed: mpsc::SyncSender<()>) -> Result<()> {
    let mut file = BufReader::new(fs::File::open(path)?);
    let mut start = 0;
    let mut primed = Some(primed);
    loop {
        let mut bytes = vec![0; 8192];
        let mut count = 0;
        while count < bytes.len() {
            let got = file.read(&mut bytes[count..])?;
            if got == 0 {
                break;
            }
            count += got;
        }
        if count == 0 {
            break;
        }
        ensure!(count % 8 == 0, "incomplete stereo audio block");
        let data = bytes[..count]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|v| f32::from_le_bytes(*v))
            .collect();
        if send.send(Block { start, data }).is_err() {
            return Ok(());
        }
        start += count as u64 / 8;
        if start >= 4096
            && let Some(ready) = primed.take()
        {
            let _ = ready.send(());
        }
    }
    if let Some(ready) = primed {
        let _ = ready.send(());
    }
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(args.seconds > 0, "positive duration required");
    if let Some(path) = &args.report {
        ensure!(!path.exists(), "choose a new report path")
    }
    let project = Project::load(&args.project)?;
    let fps = project.fps;
    let count = if args.whole_project {
        project.duration()?
    } else {
        u32::try_from(u64::from(args.seconds) * u64::from(fps.0) / u64::from(fps.1))?
    };
    ensure!(
        count > 0 && count <= project.duration()?,
        "requested playback exceeds project"
    );
    let playback_timeout =
        Duration::from_secs((u64::from(count) * u64::from(fps.1)).div_ceil(u64::from(fps.0)) + 10);
    let tmp = tempfile::tempdir()?;
    let sdl = sdl2::init().map_err(anyhow::Error::msg)?;
    let video = sdl.video().map_err(anyhow::Error::msg)?;
    let audio = sdl.audio().map_err(anyhow::Error::msg)?;
    ensure!(
        !["dummy", "offscreen"].contains(&video.current_video_driver())
            && audio.current_audio_driver() != "dummy",
        "physical display and audio drivers required"
    );
    let window = video
        .window(
            "N4 native playback trial",
            project.size[0].min(960),
            project.size[1].min(540),
        )
        .allow_highdpi()
        .position_centered()
        .build()?;
    let display = video
        .current_display_mode(window.display_index().map_err(anyhow::Error::msg)?)
        .map_err(anyhow::Error::msg)?;
    let display_name = video
        .display_name(window.display_index().map_err(anyhow::Error::msg)?)
        .map_err(anyhow::Error::msg)?;
    let mut canvas = window.into_canvas().accelerated().present_vsync().build()?;
    let (pw, ph) = canvas.window().size();
    let (dw, dh) = canvas.output_size().map_err(anyhow::Error::msg)?;
    ensure!(dw > 0 && dh > 0, "missing native drawable");
    canvas
        .window_mut()
        .set_size(project.size[0] * pw / dw, project.size[1] * ph / dh)?;
    let drawable = canvas.output_size().map_err(anyhow::Error::msg)?;
    let textures = canvas.texture_creator();
    let mut texture = textures.create_texture_streaming(
        PixelFormatEnum::RGBA32,
        project.size[0],
        project.size[1],
    )?;
    let mut events = sdl.event_pump().map_err(anyhow::Error::msg)?;
    let mut present = |image: &n4_engine::Frame| -> Result<()> {
        ensure!(
            !events.poll_iter().any(|e| matches!(e, Event::Quit { .. })),
            "window closed"
        );
        texture.update(None, &image.rgba, image.size[0] as usize * 4)?;
        canvas
            .copy(&texture, None, None)
            .map_err(anyhow::Error::msg)?;
        canvas.present();
        Ok(())
    };
    let audio_path = tmp.path().join("timeline.f32le");
    export::write_pcm(&project, &audio_path)?;
    let samples = fs::metadata(&audio_path)?.len() / 8;
    let (send, blocks) = mpsc::sync_channel(4);
    let (primed, start) = mpsc::sync_channel(1);
    let pump = thread::spawn(move || pump(audio_path, send, primed));
    start.recv().context("audio prebuffer failed")?;
    let submitted = Arc::new(AtomicU64::new(0));
    let missing = Arc::new(AtomicU64::new(0));
    let device = audio
        .open_playback(
            None,
            &AudioSpecDesired {
                freq: Some(48000),
                channels: Some(2),
                samples: Some(256),
            },
            |_| Audio {
                blocks,
                current: None,
                cursor: 0,
                total: samples,
                submitted: submitted.clone(),
                missing: missing.clone(),
            },
        )
        .map_err(anyhow::Error::msg)?;
    ensure!(
        device.spec().freq == 48000 && device.spec().channels == 2,
        "unexpected device mix format"
    );
    let quantum = u64::from(device.spec().samples);
    let mut seeks = Vec::new();
    let mut open_ms = None;
    if args.seek_count > 0 {
        let opened = Instant::now();
        let preview = Preview::open(project.clone())?;
        open_ms = Some(opened.elapsed().as_secs_f64() * 1000.);
        for i in 0..args.seek_count {
            let target = (((2 * u64::from(i) + 1) * u64::from(count))
                / (2 * u64::from(args.seek_count))) as u32;
            let mut row = vec![];
            for cold in [true, false] {
                if !cold && count > 1 {
                    let elsewhere = (target + count / 2) % count;
                    present(&preview.wait(preview.request(elsewhere))?)?;
                }
                let now = Instant::now();
                let ticket = if cold {
                    preview.cold_request(target)
                } else {
                    preview.request(target)
                };
                let frame = preview.wait(ticket)?;
                ensure!(
                    frame.trace.request.timeline_frame == target,
                    "wrong seek ticket"
                );
                present(&frame)?;
                row.push(now.elapsed().as_secs_f64() * 1000.);
            }
            seeks.push(json!({"frame":target,"fresh_ms":row[0],"repeat_ms":row[1]}));
        }
    }
    let playback = Playback::open(project.clone(), count)?;
    let first = playback.receive()?;
    present(&first)?;
    drop(first);
    // Prime the first moving picture before the device starts requesting samples.
    let mut ready = if count > 1 {
        Some(playback.receive()?)
    } else {
        None
    };
    let clock = || submitted.load(Ordering::Acquire).saturating_sub(quantum);
    let frame_at =
        |sample: u64| -> u32 { (sample * u64::from(fps.0) / (48_000 * u64::from(fps.1))) as u32 };
    let wall = Instant::now();
    device.resume();
    let mut n = 1;
    let mut on_time = vec![false; count as usize];
    on_time[0] = true;
    let mut presented = vec![];
    while n < count && frame_at(clock()) < count {
        let image = match ready.take() {
            Some(frame) => frame,
            None => playback.receive()?,
        };
        ensure!(
            image.trace.request.timeline_frame == n,
            "wrong playback frame"
        );
        if frame_at(clock()) > n {
            n += 1;
            continue;
        }
        let at = export::sample_boundary(fps, n)?;
        while clock() < at {
            ensure!(wall.elapsed() < playback_timeout, "audio clock stalled");
            thread::sleep(Duration::from_micros(400));
        }
        present(&image)?;
        let sample = clock();
        let slot = frame_at(sample);
        on_time[n as usize] = slot == n;
        presented.push((n, sample));
        n += 1;
    }
    let end_sample = export::sample_boundary(fps, count)?;
    while clock() < end_sample {
        ensure!(
            wall.elapsed() < playback_timeout,
            "audio clock stalled at end"
        );
        thread::sleep(Duration::from_millis(1));
    }
    device.pause();
    let max_miss_run = on_time
        .split(|&good| good)
        .map(<[bool]>::len)
        .max()
        .unwrap_or(0);
    let missed = on_time.iter().filter(|&&good| !good).count();
    let receipt = json!({
        "display":display_name,"display_mode":[display.w,display.h,display.refresh_rate],
        "drawable_pixels":[drawable.0, drawable.1],
        "video_driver":video.current_video_driver(),"audio_driver":audio.current_audio_driver(),
        "enumerated_audio_outputs":(0..audio.num_audio_playback_devices().unwrap_or(0)).filter_map(|i|audio.audio_playback_device_name(i).ok()).collect::<Vec<_>>(),
        "audio_callback_frames":quantum,"app_audio_starved_samples":missing.load(Ordering::Relaxed),
        "project_size":project.size,"fps":fps,"scheduled_frames":count,"missed_slots":missed,"longest_miss_run":max_miss_run,
        "seconds_wall":wall.elapsed().as_secs_f64(),"indexed_project_open_ms":open_ms,"seek_present_return":seeks,
        "present_return_audio_frame_pairs":presented,"boundary":"SDL present return relative to audio callback demand, corrected by one requested callback quantum"
    });
    drop(device);
    pump.join().map_err(|_| anyhow!("audio feeder stopped"))??;
    let bytes = serde_json::to_vec(&receipt)?;
    if let Some(path) = args.report {
        use std::io::Write;
        fs::File::create_new(path)?.write_all(&bytes)?;
    }
    println!(
        "Scheduled {count} frames, missed {missed} slots, longest run {max_miss_run}, starved {} audio sample frames",
        missing.load(Ordering::Relaxed)
    );
    Ok(())
}
