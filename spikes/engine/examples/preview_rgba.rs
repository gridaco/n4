//! Stream full-quality preview pictures for an independent encoder-input digest.
use anyhow::{Result, ensure};
use clap::Parser;
use n4_engine::{Project, preview::Preview};
use std::{
    io::{self, Write},
    path::PathBuf,
};

#[derive(Parser)]
struct Args {
    project: PathBuf,
    /// Inspect these timeline frames in request order, including repeats and backward seeks.
    #[arg(long, value_delimiter = ',')]
    frames: Option<Vec<u32>>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let project = Project::load(&args.project)?;
    let duration = project.duration()?;
    let frames = args.frames.unwrap_or_else(|| (0..duration).collect());
    ensure!(
        frames.iter().all(|&frame| frame < duration),
        "inspection frame outside project"
    );
    let preview = Preview::open(project)?;
    let mut output = io::stdout().lock();
    for index in frames {
        let frame = preview.wait(preview.request(index))?;
        ensure!(
            frame.trace.request.timeline_frame == index,
            "wrong preview frame"
        );
        output.write_all(&frame.rgba)?;
    }
    output.flush()?;
    Ok(())
}
