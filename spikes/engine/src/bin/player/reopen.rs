//! Prepare a complete read-only snapshot while the accepted edit remains playable.
use anyhow::{Context, Result};
use n4_engine::{
    RevisionChange,
    host::{PreparedSnapshot, PreparedSound, ProjectSnapshot},
    transport::ComposedFrameCache,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
};

pub(super) struct Reopen {
    path: PathBuf,
    queued: Option<(Arc<ProjectSnapshot>, Option<PreparedSound>)>,
    loading: Option<Receiver<Result<(RevisionChange, PreparedSnapshot)>>>,
    preparing_picture: Option<(RevisionChange, PreparedSnapshot, Option<u32>)>,
}

impl Reopen {
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
            queued: None,
            loading: None,
            preparing_picture: None,
        }
    }

    pub fn request(
        &mut self,
        accepted: &Arc<ProjectSnapshot>,
        sound: Option<&PreparedSound>,
    ) -> Result<()> {
        self.queued = Some((Arc::clone(accepted), sound.cloned()));
        self.preparing_picture = None;
        if self.loading.is_none() {
            self.start()?;
        }
        Ok(())
    }

    fn start(&mut self) -> Result<()> {
        let path = self.path.clone();
        let (previous, sound) = self.queued.take().context("missing queued revision")?;
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("n4-reopen".into())
            .spawn(move || {
                let _ = tx.send(PreparedSnapshot::reload(&path, &previous, sound.as_ref()));
            })?;
        self.loading = Some(rx);
        Ok(())
    }

    pub fn poll(
        &mut self,
        cursor: u32,
        frame_cache: Option<&ComposedFrameCache>,
    ) -> Result<Option<(RevisionChange, PreparedSnapshot, n4_engine::Frame)>> {
        if let Some(rx) = &self.loading {
            let result = match rx.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return Ok(None),
                Err(TryRecvError::Disconnected) => {
                    Err(anyhow::anyhow!("project reopening stopped"))
                }
            };
            self.loading.take();
            if self.queued.is_some() {
                self.start()?;
                return Ok(None);
            }
            let (change, candidate) = result?;
            self.preparing_picture = Some((change, candidate, None));
        }
        let Some((change, candidate, requested_frame)) = &mut self.preparing_picture else {
            return Ok(None);
        };
        let result = (|| {
            let target = cursor.min(candidate.inspection.duration - 1);
            if *requested_frame != Some(target) {
                candidate.transport.seek(target)?;
                *requested_frame = Some(target);
            }
            let frame = candidate.transport.try_next()?;
            if let Some(picture) = &frame {
                candidate.finish_picture(picture, target, change, frame_cache)?;
            }
            Ok(frame)
        })();
        match result {
            Ok(Some(frame)) => {
                let (change, candidate, _) = self.preparing_picture.take().unwrap();
                Ok(Some((change, candidate, frame)))
            }
            Ok(None) => Ok(None),
            Err(error) => {
                let (_, candidate, _) = self.preparing_picture.take().unwrap();
                Err(anyhow::Error::context(
                    error,
                    candidate.snapshot.inputs.clone(),
                ))
            }
        }
    }
    pub fn pending(&self) -> bool {
        self.loading.is_some() || self.preparing_picture.is_some()
    }
}

#[cfg(all(test, n4_openusd))]
mod tests {
    use super::*;
    use n4_engine::Frame;
    use std::{
        fs,
        time::{Duration, Instant},
    };

    fn arrived(reopen: &mut Reopen, accepted: &PreparedSnapshot) -> Result<PreparedSnapshot> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some((change, snapshot, picture)) =
                reopen.poll(0, Some(&accepted.transport.frame_cache()))?
            {
                assert_eq!(change.to_revision, picture.trace.revision);
                return Ok(snapshot);
            }
            anyhow::ensure!(Instant::now() < deadline, "timed out reopening");
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn picture(snapshot: &PreparedSnapshot, frame: u32) -> Result<Frame> {
        snapshot.transport.seek(frame)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(picture) = snapshot.transport.try_next()? {
                return Ok(picture);
            }
            anyhow::ensure!(Instant::now() < deadline, "timed out seeking");
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn texture(path: &Path, rgb: [u8; 3]) -> Result<()> {
        let mut encoder = png::Encoder::new(fs::File::create(path)?, 32, 32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()?
            .write_image_data(&rgb.repeat(32 * 32))?;
        Ok(())
    }

    #[test]
    fn stale_light_texture_and_display_reopens_never_replace_the_current_revision() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dir = temp.path();
        fs::create_dir(dir.join("sequences"))?;
        fs::write(
            dir.join("project.proj"),
            "project panel fps 24/1 size 128x80\n[0,4) V1 shot\n",
        )?;
        fs::write(
            dir.join("card.usda"),
            r#"#usda 1.0
(
    metersPerUnit = 1
    upAxis = "Y"
)
def Mesh "Card" (prepend apiSchemas = ["MaterialBindingAPI"]) {
    uniform token subdivisionScheme = "none"
    point3f[] points = [(-0.5,-0.5,0),(0.5,-0.5,0),(0.5,0.5,0),(-0.5,0.5,0)]
    int[] faceVertexCounts = [4]
    int[] faceVertexIndices = [0,1,2,3]
    texCoord2f[] primvars:st = [(0,0),(1,0),(1,1),(0,1)] (interpolation = "vertex")
    rel material:binding = </Finish>
}
def Material "Finish" {
    token outputs:surface.connect = </Finish/P.outputs:surface>
    def Shader "P" {
        uniform token info:id = "UsdPreviewSurface"
        color3f inputs:diffuseColor = (0.5,0.5,0.5)
        token outputs:surface
    }
}
"#,
        )?;
        let detail = dir.join("sequences/shot.xml");
        let plate = dir.join("ink.png");
        let scene = r##"<sequence camera="eye" background="#080808">
            <camera id="eye" projection="perspective" position="0 0 3" fov-y="37"/>
            <scene3d pixels-per-meter="100"><studio environment-intensity="0.3"/>
            <distant-light direction="0 0 -1" intensity="1.5"/>
            <usd source="card.usda" prim="/Card" position="-0.6 0 0"><surface slot="/Finish" role="base-color" size="32 32"><image source="ink.png"/></surface></usd>
            <usd source="card.usda" prim="/Card" position="0.6 0 0"><surface slot="/Finish" role="unlit" size="32 32"><rect width="32" height="32" fill="#247fb1"/></surface></usd>
            </scene3d></sequence>"##;
        texture(&plate, [90, 110, 130])?;
        fs::write(&detail, scene)?;
        let mut reopen = Reopen::new(dir);
        let mut accepted = PreparedSnapshot::load(dir, 0)?;
        let mut prior = picture(&accepted, 2)?;
        let relit = scene.replace("intensity=\"1.5\"", "intensity=\"0.15\"");
        for step in 0..3 {
            // Schedule a previous picture and a reload. Supersede both with a new snapshot.
            accepted.transport.play_from(0)?;
            reopen.request(&accepted.snapshot, Some(&accepted.sound))?;
            match step {
                0 => fs::write(&detail, &relit)?,
                1 => texture(&plate, [189, 32, 48])?,
                _ => fs::write(&detail, relit.replace("#247fb1", "#f0bc33"))?,
            }
            reopen.request(&accepted.snapshot, Some(&accepted.sound))?;
            accepted = arrived(&mut reopen, &accepted)?;
            let current = picture(&accepted, 2)?;
            assert_eq!(current.trace.revision, prior.trace.revision + 1);
            assert_ne!(
                current.rgba, prior.rgba,
                "edit {step} had no visible effect"
            );
            assert_eq!(current.rgba, picture(&accepted, 0)?.rgba);
            prior = current;
        }
        fs::write(
            &detail,
            relit.replace("slot=\"/Finish\"", "slot=\"/Renamed\""),
        )?;
        reopen.request(&accepted.snapshot, Some(&accepted.sound))?;
        let error = arrived(&mut reopen, &accepted)
            .err()
            .context("missing material binding was accepted")?;
        assert!(format!("{error:#}").contains("no material slot"));
        assert_eq!(prior.rgba, picture(&accepted, 3)?.rgba);
        // These fail at later preparation stages. Neither may publish a new picture.
        fs::write(&detail, &relit)?;
        fs::write(&plate, b"broken external image")?;
        reopen.request(&accepted.snapshot, Some(&accepted.sound))?;
        let error = arrived(&mut reopen, &accepted)
            .err()
            .context("invalid image accepted")?;
        assert!(format!("{error:#}").contains("ink.png"), "{error:#}");
        assert_eq!(prior.rgba, picture(&accepted, 2)?.rgba);
        texture(&plate, [189, 32, 48])?;
        fs::write(
            dir.join("project.proj"),
            "project panel fps 24/1 size 128x80\n[0,4) V1 shot\n[0,4) A1 broken\n",
        )?;
        fs::write(
            dir.join("sequences/broken.json"),
            r#"{"source":"missing.wav","source_in_sample":0}"#,
        )?;
        reopen.request(&accepted.snapshot, Some(&accepted.sound))?;
        let error = arrived(&mut reopen, &accepted)
            .err()
            .context("invalid audio accepted")?;
        assert!(format!("{error:#}").contains("missing.wav"), "{error:#}");
        assert_eq!(prior.rgba, picture(&accepted, 0)?.rgba);
        Ok(())
    }
}
