use anyhow::Result;
use n4_engine::{Engine, Project};
use serde_json::json;

#[test]
fn a_still_has_its_own_local_motion_and_reuses_its_pixels_on_a_reverse_seek() -> Result<()> {
    let folder = tempfile::tempdir()?;
    let image_path = folder.path().join("swatch.png");
    let mut encoder = png::Encoder::new(std::fs::File::create(&image_path)?, 4, 2);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut png = encoder.write_header()?;
    png.write_image_data(&[255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0].repeat(2))?;
    png.finish()?;
    let edit = folder.path().join("motion.json");
    std::fs::write(
        &edit,
        serde_json::to_vec(&json!({
            "size":[24,12],"fps":[30,1],
            "shots":[{"duration":1,"background":{"color":[0,0,0]}},
                     {"duration":6,"background":{"color":[0,0,0]},"graphics":[{
                "name":"Swatch","span":[1,6],"position":[2,2],
                "content":{"type":"image","source":"swatch.png"},
                "motion":[{"frame":0,"opacity":0},{"frame":4,"x":8,"scale":2,"opacity":1}]
            }]}]
        }))?,
    )?;
    let mut engine = Engine::new(Project::load(&edit)?)?;
    let channel = |pixels: &[u8], x: usize, y: usize| pixels[(y * 24 + x) * 4];
    assert_eq!(channel(&engine.render(2)?.rgba, 2, 2), 0);
    let halfway = engine.render(4)?.rgba;
    assert!((185..=190).contains(&channel(&halfway, 7, 3)));
    assert_eq!(channel(&halfway, 12, 3), 0);
    let end = engine.render(6)?.rgba;
    assert_eq!(channel(&end, 11, 3), 255);
    assert_eq!(channel(&end, 18, 3), 0);
    assert_eq!(engine.render(4)?.rgba.as_slice(), halfway.as_slice());
    Ok(())
}
