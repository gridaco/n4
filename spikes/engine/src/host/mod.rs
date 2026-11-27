//! Read-only viewer state. Native and embedded presenters use the same audio clock.
mod controller;
mod sound;

pub use controller::Controller;
pub use sound::{PreparedSound, Sound};

use crate::{
    AuthoringDocument, Frame, RevisionChange,
    graphics::{PreparedImageAssets, PreparedSvgAssets, PreparedTextLayouts},
    inspect::Inspection,
    preview::{ClipThumbnails, Filmstrip, Thumbnail},
    project_watch::ProjectInputs,
    transport::{ComposedFrameCache, ExportJob, FrameCacheStats, Transport},
};
use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

/// The document and dependency observations are immutable and can be shared with
/// a reload worker while the original transport continues to present frames.
pub struct ProjectSnapshot {
    pub document: AuthoringDocument,
    pub inputs: ProjectInputs,
    pub revision: u64,
    pub(crate) svg_assets: Arc<PreparedSvgAssets>,
    pub(crate) image_assets: PreparedImageAssets,
    pub(crate) text_layouts: Arc<PreparedTextLayouts>,
    audio_preparations: Arc<AtomicU64>,
}

impl ProjectSnapshot {
    pub fn load(path: &Path, revision: u64) -> Result<Arc<Self>> {
        let (mut document, inputs) = ProjectInputs::load(path)?;
        let svg_assets = PreparedSvgAssets::prepare(&document.project, None, &inputs)
            .map_err(|error| error.context(inputs.clone()))?;
        let image_assets = PreparedImageAssets::prepare(&document.project, None, &inputs)
            .map_err(|error| error.context(inputs.clone()))?;
        let text_layouts = PreparedTextLayouts::prepare(&mut document.project, None, &inputs)
            .map_err(|error| error.context(inputs.clone()))?;
        inputs
            .check()
            .map_err(|error| error.context(inputs.clone()))?;
        Ok(Arc::new(Self {
            document,
            inputs,
            revision,
            svg_assets: Arc::new(svg_assets),
            image_assets,
            text_layouts: Arc::new(text_layouts),
            audio_preparations: Arc::default(),
        }))
    }

    pub fn prepare_sound(&self) -> Result<PreparedSound> {
        PreparedSound::prepare(&self.document.project, &self.audio_preparations)
    }

    pub fn open_transport(&self) -> Result<Transport> {
        Transport::open_with_artwork(
            self.document.project.clone(),
            self.revision,
            Some(Arc::clone(&self.svg_assets)),
            Some(Arc::clone(&self.text_layouts)),
        )
    }

    pub fn open_filmstrip(&self, width: u32) -> Result<Filmstrip> {
        Filmstrip::open_with_artwork(
            self.document.project.clone(),
            width,
            Default::default(),
            Some(Arc::clone(&self.svg_assets)),
            Some(Arc::clone(&self.text_layouts)),
        )
    }
}

pub struct PreparedSnapshot {
    pub snapshot: Arc<ProjectSnapshot>,
    pub inspection: Inspection,
    pub transport: Transport,
    pub sound: PreparedSound,
}

impl PreparedSnapshot {
    pub fn load(path: &Path, revision: u64) -> Result<Self> {
        Self::prepare(ProjectSnapshot::load(path, revision)?, None)
    }

    pub fn reload(
        path: &Path,
        previous: &ProjectSnapshot,
        previous_sound: Option<&PreparedSound>,
    ) -> Result<(RevisionChange, Self)> {
        let (document, inputs) = ProjectInputs::load(path)?;
        Self::candidate(document, inputs, previous, previous_sound)
    }

    pub(crate) fn candidate(
        mut document: AuthoringDocument,
        inputs: ProjectInputs,
        previous: &ProjectSnapshot,
        previous_sound: Option<&PreparedSound>,
    ) -> Result<(RevisionChange, Self)> {
        let change = RevisionChange::between(
            previous.revision,
            &previous.document,
            &previous.inputs,
            &document,
            &inputs,
        )?;
        let sound = previous_sound
            .filter(|_| change.damage.audio.is_empty())
            .cloned();
        let svg_assets = PreparedSvgAssets::prepare(
            &document.project,
            Some((&previous.svg_assets, &previous.inputs)),
            &inputs,
        )
        .map_err(|error| error.context(inputs.clone()))?;
        let image_assets = PreparedImageAssets::prepare(
            &document.project,
            Some((&previous.image_assets, &previous.inputs)),
            &inputs,
        )
        .map_err(|error| error.context(inputs.clone()))?;
        let text_layouts = PreparedTextLayouts::prepare(
            &mut document.project,
            Some((&previous.text_layouts, &previous.inputs)),
            &inputs,
        )
        .map_err(|error| error.context(inputs.clone()))?;
        let snapshot = Arc::new(ProjectSnapshot {
            document,
            inputs,
            revision: change.to_revision,
            svg_assets: Arc::new(svg_assets),
            image_assets,
            text_layouts: Arc::new(text_layouts),
            audio_preparations: Arc::clone(&previous.audio_preparations),
        });
        Ok((change, Self::prepare(snapshot, sound)?))
    }

    fn prepare(
        snapshot: Arc<ProjectSnapshot>,
        reusable_sound: Option<PreparedSound>,
    ) -> Result<Self> {
        let prepare = || -> Result<_> {
            let inspection = snapshot.document.inspection()?;
            let sound = match reusable_sound {
                Some(sound) => sound,
                None => snapshot.prepare_sound()?,
            };
            let transport = snapshot.open_transport()?;
            Ok((inspection, sound, transport))
        };
        let (inspection, sound, transport) =
            prepare().map_err(|error| error.context(snapshot.inputs.clone()))?;
        Ok(Self {
            snapshot,
            inspection,
            sound,
            transport,
        })
    }

    /// A fresh render validates the candidate before it can inherit cached pictures.
    pub fn finish_picture(
        &self,
        picture: &Frame,
        frame: u32,
        change: &RevisionChange,
        previous: Option<&ComposedFrameCache>,
    ) -> Result<()> {
        ensure!(
            picture.trace.request.timeline_frame == frame
                && picture.trace.revision == self.snapshot.revision,
            "candidate picture does not match the reopened project"
        );
        if let Some(previous) = previous {
            self.transport.inherit_frame_cache(previous, change)?;
        }
        self.snapshot.inputs.check()
    }
}

/// Resources for one accepted revision.
pub struct Viewer {
    pub snapshot: Arc<ProjectSnapshot>,
    pub inspection: Inspection,
    pub controller: Controller,
    pub prepared: PreparedSound,
    strip: Option<Arc<Filmstrip>>,
    clip_thumbnails: Option<Arc<ClipThumbnails>>,
}

enum ThumbnailWorker {
    Filmstrip(Arc<Filmstrip>),
    Placement(String, Arc<ClipThumbnails>),
}

/// The pending request retains its original worker and revision across clean transitions.
/// The sidecar has one such slot. A new request or invalidating edit releases it.
pub struct ThumbnailTicket {
    pub revision: u64,
    sequence: u64,
    worker: ThumbnailWorker,
}

impl ThumbnailTicket {
    pub fn survives(&self, change: &RevisionChange, frame: u32) -> bool {
        matches!(&self.worker, ThumbnailWorker::Placement(path, _) if change.timeline.preserves_thumbnail(path, frame))
    }

    pub fn try_take(&self) -> Result<Option<Thumbnail>> {
        match &self.worker {
            ThumbnailWorker::Filmstrip(worker) => worker.try_take(self.sequence),
            ThumbnailWorker::Placement(_, worker) => worker.try_take(self.sequence),
        }
    }
}

impl Viewer {
    pub fn open(path: &Path, revision: u64) -> Result<Self> {
        let next = PreparedSnapshot::load(path, revision)?;
        let mut controller = Controller::new(next.inspection.duration, next.inspection.fps);
        if let Err(error) = controller.attach_transport(next.transport) {
            return Err(error.context(next.snapshot.inputs.clone()));
        }
        Ok(Self {
            snapshot: next.snapshot,
            inspection: next.inspection,
            prepared: next.sound,
            controller,
            strip: None,
            clip_thumbnails: None,
        })
    }

    pub fn frame_cache_stats(&self) -> FrameCacheStats {
        self.controller
            .frame_cache()
            .expect("open viewer has a transport")
            .stats()
    }

    pub fn audio_preparations(&self) -> u64 {
        self.snapshot.audio_preparations.load(Ordering::Relaxed)
    }

    pub fn check_frame(&self, frame: u32) -> Result<()> {
        ensure!(
            frame < self.inspection.duration,
            "frame {frame} outside project"
        );
        Ok(())
    }

    pub fn reopen(
        &mut self,
        path: &Path,
        desired_frame: Option<u32>,
    ) -> Result<(Frame, RevisionChange)> {
        let (change, next) = PreparedSnapshot::reload(path, &self.snapshot, Some(&self.prepared))?;
        self.accept_candidate(next, change, desired_frame)
    }

    pub(crate) fn accept_candidate(
        &mut self,
        next: PreparedSnapshot,
        change: RevisionChange,
        desired_frame: Option<u32>,
    ) -> Result<(Frame, RevisionChange)> {
        let cursor = desired_frame
            .or(self.controller.displayed)
            .unwrap_or(self.controller.cursor)
            .min(next.inspection.duration - 1);
        let park = || -> Result<Frame> {
            next.transport.seek(cursor)?;
            loop {
                if let Some(picture) = next.transport.try_next()? {
                    next.finish_picture(
                        &picture,
                        cursor,
                        &change,
                        self.controller.frame_cache().as_ref(),
                    )?;
                    return Ok(picture);
                }
                thread::sleep(Duration::from_millis(1));
            }
        };
        let picture = park().map_err(|error| error.context(next.snapshot.inputs.clone()))?;
        let sound = if change.damage.audio.is_empty() {
            self.controller.take_sound()
        } else {
            None
        };
        *self = Self {
            controller: Controller::reopened(
                next.inspection.duration,
                next.inspection.fps,
                cursor,
                next.transport,
                sound,
            ),
            snapshot: next.snapshot,
            inspection: next.inspection,
            prepared: next.sound,
            strip: None,
            clip_thumbnails: None,
        };
        Ok((picture, change))
    }

    /// A device is needed only when the user plays. Headless inspection and
    /// scrubbing still work on machines without an active sound output.
    pub fn play(&mut self, frame: u32) -> Result<()> {
        self.check_frame(frame)?;
        if !self.controller.ready() {
            let sdl = sdl2::init().map_err(anyhow::Error::msg)?;
            let audio = sdl.audio().map_err(anyhow::Error::msg)?;
            self.controller
                .attach_sound(Sound::open(&audio, &self.prepared)?);
        }
        self.controller.play_at(frame)
    }

    pub fn seek(&mut self, frame: u32) -> Result<()> {
        self.check_frame(frame)?;
        self.controller.seek_exact(frame)
    }

    pub fn poll(&mut self) -> Result<Option<Frame>> {
        self.controller.poll()
    }

    pub fn export_mp4(&mut self, path: PathBuf) -> Result<ExportJob> {
        self.controller.export_mp4(path, self.prepared.mix_path())
    }

    pub fn request_thumbnail(
        &mut self,
        frame: u32,
        placement: Option<&str>,
    ) -> Result<ThumbnailTicket> {
        self.check_frame(frame)?;
        if let Some(path) = placement {
            let occurrence = self
                .inspection
                .placement(path)
                .context("unknown thumbnail placement")?;
            if self.clip_thumbnails.is_none() {
                self.clip_thumbnails = Some(Arc::new(ClipThumbnails::for_project(
                    self.snapshot.document.project.clone(),
                    &self.inspection.placements,
                    Some(Arc::clone(&self.snapshot.svg_assets)),
                    Some(Arc::clone(&self.snapshot.text_layouts)),
                )?));
            }
            let worker = self.clip_thumbnails.as_ref().unwrap();
            return Ok(ThumbnailTicket {
                revision: self.snapshot.revision,
                sequence: worker.request_placement(frame, occurrence)?,
                worker: ThumbnailWorker::Placement(path.into(), Arc::clone(worker)),
            });
        }
        if self.strip.is_none() {
            self.strip = Some(Arc::new(self.snapshot.open_filmstrip(120)?));
        }
        let worker = self.strip.as_ref().unwrap();
        Ok(ThumbnailTicket {
            revision: self.snapshot.revision,
            sequence: worker.request(frame),
            worker: ThumbnailWorker::Filmstrip(Arc::clone(worker)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::Instant};

    fn await_picture<T>(mut poll: impl FnMut() -> Result<Option<T>>) -> Result<T> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(picture) = poll()? {
                return Ok(picture);
            }
            ensure!(Instant::now() < deadline, "picture did not complete");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn cold_clip_thumbnail_uses_the_accepted_svg_after_a_rejected_reopen() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let project = temp.path().join("project.json");
        let svg = temp.path().join("mark.svg");
        fs::write(
            &project,
            r#"{
                "size":[32,16], "fps":[24,1],
                "shots":[{"duration":8,"background":{"color":[0,0,0]},"graphics":[
                    {"content":{"type":"svg","source":"mark.svg"}}
                ]}]
            }"#,
        )?;
        fs::write(
            &svg,
            r##"<svg width="32" height="16"><path d="M0 0 H32 V16 H0 Z" fill="#ff0000"/></svg>"##,
        )?;
        let mut viewer = Viewer::open(&project, 0)?;
        let original = Arc::clone(&viewer.snapshot.svg_assets);
        viewer.reopen(&project, Some(0))?;
        assert!(std::ptr::eq(
            original.get(&svg).unwrap(),
            viewer.snapshot.svg_assets.get(&svg).unwrap(),
        ));

        fs::write(&svg, "invalid SVG")?;
        let error = viewer
            .reopen(&project, Some(0))
            .err()
            .context("accepted a broken SVG")?;
        assert!(format!("{error:#}").contains("parse SVG XML"));
        let shot = viewer.inspection.placements[0].path.clone();
        let ticket = viewer.request_thumbnail(4, Some(&shot))?;
        let picture = await_picture(|| ticket.try_take())?;
        assert_eq!(ticket.revision, 1);
        assert!(picture.rgba.chunks_exact(4).all(|p| p == [255, 0, 0, 255]));
        Ok(())
    }

    #[test]
    fn reopen_rejects_a_broken_title_font_before_the_title_appears() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let project = temp.path().join("project.json");
        let font = temp.path().join("title.ttf");
        let original = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fonts/N4GridSans.ttf");
        fs::copy(&original, &font)?;
        fs::write(
            &project,
            r#"{
                "size":[64,32], "fps":[24,1],
                "shots":[{"duration":8,"background":{"color":[0,0,0]}}],
                "title":{"text":"N4","font":"title.ttf","font_size":16,"width":60,
                         "position":[0,0],"start":4,"fade_frames":0}
            }"#,
        )?;
        let mut viewer = Viewer::open(&project, 0)?;
        let shot = viewer.inspection.placements[0].path.clone();
        let thumbnail = viewer.request_thumbnail(4, Some(&shot))?;
        let pixels = await_picture(|| thumbnail.try_take())?.rgba;
        assert!(pixels.chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 255]));

        fs::write(&font, "invalid font")?;
        let error = viewer
            .reopen(&project, Some(0))
            .err()
            .context("accepted a broken title font")?;
        assert!(format!("{error:#}").contains("decode paragraph font"));
        assert_eq!(viewer.snapshot.revision, 0);

        viewer.seek(4)?;
        let picture = await_picture(|| viewer.poll())?;
        assert_eq!(picture.trace.request.timeline_frame, 4);
        assert_eq!(picture.trace.revision, 0);
        assert_eq!(picture.trace.paragraph_lines, Some(1));

        fs::copy(original, font)?;
        let (parked, change) = viewer.reopen(&project, Some(0))?;
        assert_eq!(parked.trace.request.timeline_frame, 0);
        assert_eq!(change.to_revision, 1);
        Ok(())
    }

    #[test]
    fn pending_thumbnail_survives_a_clean_reopen_and_expires_on_a_dirty_one() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let project = temp.path();
        let sequences = project.join("sequences");
        fs::create_dir(&sequences)?;
        fs::write(
            project.join("project.proj"),
            "project film fps 24/1 size 32x16\n[0,8) V1 plate\n[0,8) V2 tag\n",
        )?;
        let card = sequences.join("tag.xml");
        let plate = sequences.join("plate.xml");
        fs::write(&plate, r##"<sequence background="#123456"/>"##)?;
        let art = r##"<sequence><rect width="32" height="16" fill="#ff0000"/></sequence>"##;
        fs::write(&card, art)?;
        let mut viewer = Viewer::open(project, 0)?;
        let pending = viewer.request_thumbnail(3, Some("V2:tag@0"))?;
        fs::write(&plate, r##"<sequence background="#2468aa"/>"##)?;
        let (_, first) = viewer.reopen(project, Some(3))?;
        assert!(pending.survives(&first, 3));
        fs::write(&card, format!("{art}\n"))?;
        let (_, second) = viewer.reopen(project, Some(3))?;
        assert!(pending.survives(&second, 3));
        let pixels = await_picture(|| pending.try_take())?.rgba;
        assert_eq!(pending.revision, 0);
        assert_eq!(&pixels[..3], &[255, 0, 0]);
        let another = viewer.request_thumbnail(3, Some("V2:tag@0"))?;
        fs::write(&card, art.replace("#ff0000", "#00ff00"))?;
        let (_, dirty) = viewer.reopen(project, Some(3))?;
        assert!(!another.survives(&dirty, 3));
        Ok(())
    }
}
