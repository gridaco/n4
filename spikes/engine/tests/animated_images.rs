//! The colors and display intervals below describe the authored patches by hand.
//! Static PNG witnesses travel through the same compositor and encoder as the animation.
use anyhow::Result;
use n4_engine::{Engine, Project, export::write_mp4, preview::Filmstrip};
use std::{fs, path::Path};

struct Animation {
    filename: &'static str,
    height: u32,
    /// Exclusive end tick at 50 fps and one displayed row (identical on every scanline).
    pictures: &'static [(u32, &'static str)],
}

const WEBP: Animation = Animation {
    filename: "webp/patches.webp",
    height: 2,
    pictures: &[(2, "RR.."), (6, "..GG"), (7, "WWGG")],
};
const GIF: Animation = Animation {
    filename: "gif/restore.gif",
    height: 4,
    pictures: &[
        (2, "RRRR...."),
        (5, "RGRRG..."),
        (6, "RRRR..BB"),
        (10, "YRRR...."),
    ],
};

fn color(symbol: u8) -> [u8; 4] {
    match symbol {
        b'.' => [0; 4],
        b'R' => [255, 0, 0, 255],
        b'G' => [0, 255, 0, 255],
        b'B' => [0, 0, 255, 255],
        b'Y' => [255, 255, 0, 255],
        b'W' => [255; 4],
        _ => unreachable!(),
    }
}

impl Animation {
    fn width(&self) -> u32 {
        self.pictures[0].1.len() as u32
    }

    fn duration(&self) -> u32 {
        self.pictures.last().unwrap().0
    }

    fn pixels(&self, tick: u32) -> Vec<u8> {
        let (_, row) = self.pictures.iter().find(|(end, _)| tick < *end).unwrap();
        row.bytes()
            .flat_map(color)
            .collect::<Vec<_>>()
            .repeat(self.height as usize)
    }

    fn project(&self, root: &Path, shot: &str) -> Result<Project> {
        fs::create_dir_all(root.join("sequences"))?;
        fs::write(
            root.join("project.proj"),
            format!(
                "project alpha fps 50/1 size {}x{}\n[0,{}) V1 test\n",
                self.width(),
                self.height,
                self.duration()
            ),
        )?;
        fs::write(root.join("sequences/test.xml"), shot)?;
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join(self.filename),
            root.join("media.bin"),
        )?;
        Project::load(root)
    }
}

fn over_black(mut pixels: Vec<u8>) -> Vec<u8> {
    for pixel in pixels.chunks_exact_mut(4) {
        if pixel[3] == 0 {
            pixel.copy_from_slice(&[0, 0, 0, 255]);
        }
    }
    pixels
}

fn check_scrub_and_export(fixture: &Animation) -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let anim = tmp.path().join("animation");
    let (width, height, duration) = (fixture.width(), fixture.height, fixture.duration());
    let half_width = width / 2;
    let last = duration - 1;
    let shot = |art: &str| {
        format!(
            r##"<sequence background="#ffffff">
          <rect width="{half_width}" height="{height}" fill="#000000">
            <animate attributeName="translate-x"><key frame="0" value="0"/><key frame="{last}" value="{half_width}"/></animate>
          </rect>
          {art}
        </sequence>"##
        )
    };
    let layer = format!(
        r#"<video source="media.bin" start="0" source-in="0" duration="{duration}" size="{width} {height}"/>"#
    );
    let mut engine = Engine::new(fixture.project(&anim, &shot(&layer))?)?;
    let expected = tmp.path().join("expected");
    fs::create_dir(&expected)?;
    let mut layers = String::new();
    let mut start = 0;
    for (i, &(end, _)) in fixture.pictures.iter().enumerate() {
        let name = format!("witness-{i}.png");
        let mut png = png::Encoder::new(fs::File::create(expected.join(&name))?, width, height);
        png.set_color(png::ColorType::Rgba);
        png.set_depth(png::BitDepth::Eight);
        png.write_header()?
            .write_image_data(&fixture.pixels(start))?;
        layers.push_str(&format!(r#"<image span="{start} {end}" source="{name}"/>"#));
        start = end;
    }
    let mut control = Engine::new(fixture.project(&expected, &shot(&layers))?)?;
    let strip = Filmstrip::open(engine.project().clone(), width)?;
    // First a cold seek straight to the last picture, then forward and backward across every hold.
    for t in [last]
        .into_iter()
        .chain(0..duration)
        .chain((0..duration).rev())
    {
        let pixels = control.render(t)?.rgba;
        assert_eq!(engine.render(t)?.rgba, pixels, "scrub to {t}");
        assert_eq!(
            strip.wait(strip.request(t))?.rgba,
            pixels,
            "composed thumbnail {t}"
        );
    }
    assert_eq!(
        write_mp4(&mut engine, &anim.join("cut.mp4"))?.rgba_sha256,
        write_mp4(&mut control, &expected.join("cut.mp4"))?.rgba_sha256
    );
    Ok(())
}

#[test]
fn webp_scrub_disposal_and_export_match_independent_pictures() -> Result<()> {
    check_scrub_and_export(&WEBP)
}

#[test]
fn gif_scrub_disposal_and_export_match_independent_pictures() -> Result<()> {
    check_scrub_and_export(&GIF)
}

#[test]
fn transparent_primary_resolves_over_black_and_source_thumbnails_keep_alpha() -> Result<()> {
    for fixture in [&GIF, &WEBP] {
        let tmp = tempfile::tempdir()?;
        let project = fixture.project(tmp.path(), r#"<sequence source="media.bin"/>"#)?;
        let strip = Filmstrip::open(project.clone(), fixture.width())?;
        let mut engine = Engine::new(project)?;
        fs::remove_file(tmp.path().join("media.bin"))?;
        for t in (0..fixture.duration()).rev() {
            let expected = fixture.pixels(t);
            let thumb = strip.wait(strip.request(t))?;
            let ordinal = fixture
                .pictures
                .iter()
                .position(|(end, _)| t < *end)
                .unwrap();
            assert_eq!(thumb.source_frame, ordinal as u32);
            assert_eq!(&*thumb.rgba, &expected, "source thumbnail {t}");
            assert_eq!(
                &*engine.render(t)?.rgba,
                &over_black(expected),
                "base picture {t}"
            );
        }
    }
    Ok(())
}

#[cfg(feature = "host-audio")]
#[test]
fn broken_replacement_keeps_accepted_animation_for_cold_workers() -> Result<()> {
    use n4_engine::editor::{EditorCommand, EditorSession};
    use std::time::{Duration, Instant};
    for fixture in [&GIF, &WEBP] {
        let tmp = tempfile::tempdir()?;
        fixture.project(tmp.path(), r#"<sequence source="media.bin"/>"#)?;
        let mut session = EditorSession::open(tmp.path())?;
        let media = tmp.path().join("media.bin");
        let original = fs::read(&media)?;
        // Damage only the final payload. Probing the first frame still succeeds.
        let mut corrupt = original.clone();
        if original.starts_with(b"GIF") {
            let control = original
                .windows(3)
                .rposition(|b| b == [0x21, 0xf9, 4])
                .unwrap();
            let lzw_code_size = control + 8 + 10;
            corrupt[lzw_code_size] = 12;
        } else {
            let image = original.windows(4).rposition(|b| b == b"VP8L").unwrap();
            corrupt[image + 8] = 0;
        }
        fs::write(&media, corrupt)?;
        let reload = || EditorCommand::Reload {
            revision: 0,
            discard_changes: false,
        };
        assert!(session.dispatch(reload()).is_err());
        assert_eq!(session.state().revision, 0);
        let last = fixture.duration() - 1;
        assert_eq!(
            &*session.render(0, last)?.rgba,
            &over_black(fixture.pixels(last))
        );

        let ordinal = fixture.pictures.len() - 2;
        let tick = fixture.pictures[ordinal - 1].0;
        let row = fixture.pictures[ordinal].1.as_bytes();
        for placement in [None, Some("V1:test@0")] {
            let ticket = session.viewer_mut().request_thumbnail(tick, placement)?;
            let deadline = Instant::now() + Duration::from_secs(10);
            let thumbnail = loop {
                if let Some(image) = ticket.try_take()? {
                    break image;
                }
                anyhow::ensure!(Instant::now() < deadline, "thumbnail timed out");
                std::thread::sleep(Duration::from_millis(1));
            };
            assert_eq!(thumbnail.source_frame, ordinal as u32);
            let width = thumbnail.size[0] as usize;
            // At the clamped edges these samples remain exact at any thumbnail size.
            assert_eq!(&thumbnail.rgba[..4], &color(row[0]));
            assert_eq!(
                &thumbnail.rgba[(width - 1) * 4..width * 4],
                &color(row[row.len() - 1])
            );
        }
        fs::write(&media, original)?;
        session.dispatch(reload())?;
        assert_eq!(session.state().revision, 1);
        assert_eq!(
            &*session.render(1, last)?.rgba,
            &over_black(fixture.pixels(last))
        );
    }
    Ok(())
}
