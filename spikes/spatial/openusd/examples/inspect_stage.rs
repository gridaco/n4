//! Inspect the imported CPU asset with the same reader that N4 uses at project open.
use anyhow::{Result, ensure};
use std::path::Path;

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(args.len() == 2, "usage: inspect_stage FILE /PRIM");
    let start = std::time::Instant::now();
    let mesh = nothing_openusd::load(Path::new(&args[0]), &args[1])?;
    let elapsed = start.elapsed();
    if std::env::var_os("N4_IMPORT_TIMING").as_deref() == Some(std::ffi::OsStr::new("1")) {
        eprintln!("import_us {}", elapsed.as_micros());
    }
    println!(
        "{} triangles, {} rigid parts, {} material slots, {} images",
        mesh.indices.len() / 3,
        mesh.parts.len(),
        mesh.materials.len(),
        mesh.images.len()
    );
    for material in &mesh.materials {
        println!("slot {}", material.slot);
    }
    Ok(())
}
