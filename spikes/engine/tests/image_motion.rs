use anyhow::Result;
use n4_engine::{Engine, Project};
use serde_json::json;

#[test]
fn webp_matches_png_for_a_moving_filtered_image_in_preview_and_export() -> Result<()> {
    use n4_engine::{export::write_mp4, preview::Filmstrip};
    use std::{fs, path::Path};

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/webp");
    let folder = tempfile::tempdir()?;
    let project = |format: &str| -> Result<Project> {
        let root = folder.path().join(format);
        fs::create_dir_all(root.join("sequences"))?;
        let asset = format!("decal.{format}");
        fs::copy(fixture.join(&asset), root.join(&asset))?;
        fs::write(
            root.join("project.proj"),
            "project still fps 24/1 size 64x32\n[0,5) V1 print\n",
        )?;
        fs::write(
            root.join("sequences/print.xml"),
            format!(
                r##"<sequence background="#000000">
                      <rect x="32" width="32" height="32" fill="#ffffff"/>
                      <image source="{asset}" position="12 8">
                        <animate attributeName="translate-x">
                          <key frame="0" value="0"/><key frame="4" value="20"/>
                        </animate>
                        <source-blur sigma="1.5"/>
                      </image>
                    </sequence>"##
            ),
        )?;
        Project::load(&root)
    };
    let mut png = Engine::new(project("png")?)?;
    let webp_project = project("webp")?;
    let strip = Filmstrip::open(webp_project.clone(), 64)?;
    let mut webp = Engine::new(webp_project)?;
    let first = webp.render(0)?.rgba;
    assert_ne!(first, webp.render(4)?.rgba, "the artwork must move");
    for frame in [4, 0, 2, 1, 3] {
        let expected = png.render(frame)?.rgba;
        let actual = webp.render(frame)?;
        assert!(actual.rgba == expected, "composed frame {frame}");
        assert_eq!(actual.trace.graphics.raster_paints, 0, "warm frame {frame}");
        let thumb = strip.wait(strip.request(frame))?;
        assert!(thumb.rgba == expected, "filmstrip frame {frame}");
    }
    let png_export = write_mp4(&mut png, &folder.path().join("png.mp4"))?;
    let webp_export = write_mp4(&mut webp, &folder.path().join("webp.mp4"))?;
    assert_eq!(webp_export.frames, 5);
    assert_eq!(webp_export.rgba_sha256, png_export.rgba_sha256);
    Ok(())
}

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
