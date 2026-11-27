//! Measure full-quality frame renders without an encoder or a presentation clock.
use anyhow::{Result, ensure};
use clap::Parser;
use n4_engine::{Engine, Project};
use serde_json::json;
use std::{path::PathBuf, time::Instant};

#[derive(Parser)]
struct Args {
    project: PathBuf,
    /// Render these frames in order on one engine, retaining repeated requests.
    #[arg(long, value_delimiter = ',')]
    frames: Vec<u32>,
    /// Sample native GPU allocation bytes at 1 ms intervals during the requested frames.
    #[arg(long, requires = "frames")]
    gpu_memory: bool,
}

fn render_us(engine: &mut Engine, frame: u32) -> Result<u64> {
    let start = Instant::now();
    let picture = engine.render(frame)?;
    let micros = start.elapsed().as_micros() as u64;
    drop(picture);
    Ok(micros)
}

fn main() -> Result<()> {
    let args = Args::parse();
    let opened = Instant::now();
    let project = Project::load(&args.project)?;
    let parse_us = opened.elapsed().as_micros() as u64;
    let duration = project.duration()?;
    for &frame in &args.frames {
        ensure!(frame < duration, "frame {frame} is outside [0,{duration})");
    }
    let start = Instant::now();
    let mut engine = Engine::new(project.clone())?;
    let prepare_us = start.elapsed().as_micros() as u64;
    if !args.frames.is_empty() {
        let (stop, signal) = std::sync::mpsc::channel::<()>();
        let probe = engine.gpu_allocation_probe();
        let monitor = args.gpu_memory.then(|| {
            std::thread::spawn(move || {
                let mut peak = probe();
                while signal.recv_timeout(std::time::Duration::from_millis(1))
                    == Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                {
                    peak = peak.max(probe());
                }
                peak.max(probe())
            })
        });
        let mut samples = Vec::with_capacity(args.frames.len());
        for frame in args.frames {
            samples.push(json!({"frame": frame, "render_us": render_us(&mut engine, frame)?}));
        }
        drop(stop);
        let peak = monitor.and_then(|t| t.join().expect("allocation sampler panicked"));
        println!(
            "{}",
            json!({"adapter": engine.adapter_name(), "size": project.size,
                   "fps": project.fps, "frames": samples, "parse_us":parse_us, "engine_prepare_us":prepare_us, "peak_sampled_gpu_bytes":peak})
        );
        return Ok(());
    }
    let mut shots = Vec::new();
    let mut start = 0;
    for shot in &project.shots {
        let end = start + shot.duration;
        let mut samples = Vec::new();
        for frame in start..end {
            samples.push(render_us(&mut engine, frame)?);
        }
        shots.push(json!({"name":shot.name, "start":start, "end":end, "render_us":samples}));
        start = end;
    }
    println!(
        "{}",
        json!({"adapter":engine.adapter_name(), "size":project.size,
        "fps":project.fps, "shots":shots})
    );
    Ok(())
}
