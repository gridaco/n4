//! Observe the inputs consulted by the authoring loader. A settled change is a
//! hint to try a new candidate. The host accepts a revision only after validation.
use crate::{AuthoringDocument, Project};
use anyhow::{Context, Result, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, SystemTime},
};

/// Files observed while reading one candidate, sampled before each input's first use.
/// Keep the authored path so a rename or symlink replacement is visible to the watcher.
#[derive(Clone, Debug, Default)]
pub struct ProjectInputs {
    files: BTreeMap<PathBuf, Option<FileStamp>>,
    // One file can contribute to multiple placements and outputs.
    resources: BTreeMap<PathBuf, BTreeSet<InputOwner>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum InputOwner {
    Authoring,
    ProjectPicture,
    Shot(usize),
    Overlay(usize),
    Audio(usize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileStamp {
    size: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64),
    #[cfg(unix)]
    changed: (i64, i64),
}

fn stamp(path: &Path) -> Option<FileStamp> {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    let file = fs::metadata(path).ok()?;
    Some(FileStamp {
        size: file.len(),
        modified: file.modified().ok(),
        #[cfg(unix)]
        identity: (file.dev(), file.ino()),
        #[cfg(unix)]
        changed: (file.ctime(), file.ctime_nsec()),
    })
}

impl ProjectInputs {
    /// A rejected candidate carries its partial input set in the error chain.
    pub fn load(path: &Path) -> Result<(AuthoringDocument, Self)> {
        Self::load_candidate(path, None)
    }

    /// Evaluate a draft master while observing the same on-disk dependencies.
    pub fn load_with_master(path: &Path, bytes: &[u8]) -> Result<(AuthoringDocument, Self)> {
        Self::load_candidate(path, Some(bytes))
    }

    fn load_candidate(path: &Path, master: Option<&[u8]>) -> Result<(AuthoringDocument, Self)> {
        let path = std::path::absolute(path)?;
        let mut inputs = Self::default();
        match AuthoringDocument::load_observed(&path, None, master, &mut |path, owner| {
            inputs.observe(path);
            if owner != InputOwner::Authoring {
                inputs
                    .resources
                    .entry(path.to_owned())
                    .or_default()
                    .insert(owner);
            }
        }) {
            Ok(document) => Ok((document, inputs)),
            Err(error) => Err(error.context(inputs)),
        }
    }

    /// After a saved draft replaces the master, retain the observed dependencies
    /// and update only the committed file's observation.
    pub fn refresh_master_observation(&mut self, path: &Path) -> Result<()> {
        let path = Project::entry_path(&std::path::absolute(path)?);
        let observed = self
            .files
            .get_mut(&path)
            .with_context(|| format!("master was not observed: {}", path.display()))?;
        *observed = stamp(&path);
        Ok(())
    }

    fn observe(&mut self, path: &Path) {
        self.files.entry(path.into()).or_insert_with(|| stamp(path));
    }

    // Compare direct binary inputs independently of authoring files. The resolved
    // project already captures the meaning of an XML/JSON/master edit.
    pub(crate) fn changed_resources(&self, other: &Self) -> BTreeSet<InputOwner> {
        let mut owners = BTreeSet::new();
        for (path, uses) in &self.resources {
            let stamp = self.files.get(path).and_then(Option::as_ref);
            if stamp.is_none() || stamp != other.files.get(path).and_then(Option::as_ref) {
                owners.extend(uses);
            }
        }
        owners
    }

    pub(crate) fn observed(&self) -> bool {
        !self.files.is_empty()
    }

    #[cfg(feature = "host-audio")]
    pub(crate) fn same_file(&self, previous: &Self, path: &Path) -> bool {
        let stamp = self.files.get(path).and_then(Option::as_ref);
        stamp.is_some() && stamp == previous.files.get(path).and_then(Option::as_ref)
    }

    /// Call immediately before accepting the candidate to detect edits made during preparation.
    pub fn check(&self) -> Result<()> {
        for (path, before) in &self.files {
            ensure!(
                *before == stamp(path),
                "input changed while loading {}",
                path.display()
            );
        }
        Ok(())
    }
}

// Preserve the observed subset on failures so creating a missing leaf or asset can trigger a retry.
impl fmt::Display for ProjectInputs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("read project inputs")
    }
}

enum Baseline {
    Accepted(ProjectInputs),
    Rejected(ProjectInputs),
}

struct Observations {
    accepted_paths: BTreeSet<PathBuf>,
    seen: BTreeMap<PathBuf, Option<FileStamp>>,
    dirty: bool,
}

impl Observations {
    fn new(inputs: ProjectInputs) -> Self {
        Self {
            accepted_paths: inputs.files.keys().cloned().collect(),
            seen: inputs.files,
            dirty: false,
        }
    }

    fn reset(&mut self, update: Baseline) {
        match update {
            Baseline::Accepted(inputs) => *self = Self::new(inputs),
            Baseline::Rejected(inputs) => {
                // Keep the active project's inputs, along with the most recent attempted
                // candidate. A newly referenced missing asset can then be repaired alone.
                self.seen
                    .retain(|path, _| self.accepted_paths.contains(path));
                self.seen.extend(inputs.files);
                self.dirty = false;
            }
        }
    }

    fn scan(&mut self, pending: &AtomicBool) {
        let mut changed = false;
        for (path, previous) in &mut self.seen {
            let current = stamp(path);
            if current != *previous {
                *previous = current;
                changed = true;
            }
        }
        if changed {
            self.dirty = true;
            pending.store(false, Ordering::Relaxed);
        } else if std::mem::take(&mut self.dirty) {
            pending.store(true, Ordering::Relaxed);
        }
    }
}

/// One worker per watching session. Sampling happens off the presentation thread.
/// An unread hint persists until consumed or superseded by a newer save/baseline.
pub struct ProjectWatch {
    updates: Sender<(Baseline, Arc<AtomicBool>)>,
    pending: Arc<AtomicBool>,
}

impl ProjectWatch {
    pub fn new(inputs: &ProjectInputs) -> Result<Self> {
        Self::start(inputs, Duration::from_millis(250))
    }

    fn start(inputs: &ProjectInputs, interval: Duration) -> Result<Self> {
        let (updates, receiver) = mpsc::channel();
        let pending = Arc::new(AtomicBool::new(false));
        let mut signal = Arc::clone(&pending);
        let mut observations = Observations::new(inputs.clone());
        thread::Builder::new()
            .name("n4-project-watch".into())
            .spawn(move || {
                loop {
                    match receiver.recv_timeout(interval) {
                        Ok((update, next_signal)) => {
                            observations.reset(update);
                            signal = next_signal;
                        }
                        Err(RecvTimeoutError::Timeout) => observations.scan(&signal),
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .context("start project watcher")?;
        Ok(Self { updates, pending })
    }

    /// Leave the hint pending while an export or another reload owns the session.
    pub fn take_changed(&self) -> bool {
        self.pending.swap(false, Ordering::Relaxed)
    }

    pub fn accepted(&mut self, inputs: &ProjectInputs) {
        self.update(Baseline::Accepted(inputs.clone()));
    }

    pub fn rejected(&mut self, error: &anyhow::Error) {
        if let Some(inputs) = error.downcast_ref::<ProjectInputs>() {
            self.update(Baseline::Rejected(inputs.clone()));
        }
    }

    fn update(&mut self, update: Baseline) {
        // A scan already running against the old baseline may finish after this call.
        // Give it a separate signal so it cannot mark this newer baseline dirty.
        self.pending = Arc::new(AtomicBool::new(false));
        self.updates
            .send((update, Arc::clone(&self.pending)))
            .expect("project watcher stopped while session is open");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placed_files_and_assets_follow_the_loader_through_nested_artwork() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/field-unit");
        let (_, inputs) = ProjectInputs::load(&root)?;
        for relative in [
            "project.proj",
            "sequences/instrument.xml",
            "sequences/score.json",
            "assets/field.usdc",
            "assets/score.wav",
            "assets/N4GridSans.ttf",
            "assets/signal.mp4",
            "sequences/imprint.xml",
        ] {
            assert!(inputs.files[&root.join(relative)].is_some(), "{relative}");
        }
        assert_eq!(inputs.files[&root.join("sequences/instrument.json")], None);
        inputs.check()
    }

    #[test]
    fn missing_leaf_repair_and_same_size_atomic_replacement_invalidate_observations() -> Result<()>
    {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let entry = root.join("project.proj");
        fs::write(&entry, "project test fps 30/1 size 64x36\n[0,5) V1 new\n")?;
        let failure = ProjectInputs::load(root).err().expect("missing detail");
        let inputs = failure.downcast_ref::<ProjectInputs>().unwrap();
        let leaf = root.join("sequences/new.xml");
        assert_eq!(inputs.files[&leaf], None);
        assert_eq!(inputs.files[&root.join("sequences/new.json")], None);
        inputs.check()?;
        fs::create_dir(root.join("sequences"))?;
        fs::write(&leaf, r##"<sequence background="#eeeeee"/>"##)?;
        assert!(inputs.check().is_err());

        let (_, repaired) = ProjectInputs::load(root)?;
        repaired.check()?;
        let replacement = root.join("next");
        fs::copy(&leaf, &replacement)?;
        fs::rename(replacement, leaf)?;
        assert!(repaired.check().is_err());
        Ok(())
    }

    #[test]
    fn saves_coalesce_until_quiet_and_a_deferred_hint_is_delivered_once() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("input");
        fs::write(&path, "a")?;
        let mut inputs = ProjectInputs::default();
        inputs.observe(&path);
        let mut observations = Observations::new(inputs);
        let pending = AtomicBool::new(false);
        fs::write(temp.path().join("notes"), "unrelated")?;
        observations.scan(&pending);
        observations.scan(&pending);
        assert!(!pending.load(Ordering::Relaxed));

        for edit in ["second", "third"] {
            fs::write(&path, edit)?;
            observations.scan(&pending);
            assert!(!pending.load(Ordering::Relaxed));
        }
        observations.scan(&pending);
        observations.scan(&pending);
        assert!(pending.swap(false, Ordering::Relaxed));
        observations.scan(&pending);
        assert!(!pending.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn worker_follows_rejected_and_accepted_candidates_and_detects_save_during_load() -> Result<()>
    {
        use std::time::Instant;
        let wait = |watch: &ProjectWatch| -> Result<()> {
            let end = Instant::now() + Duration::from_secs(3);
            while !watch.take_changed() {
                ensure!(Instant::now() < end, "watcher missed a settled change");
                thread::sleep(Duration::from_millis(2));
            }
            Ok(())
        };
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let entry = root.join("project.proj");
        let write_master = |name: &str| {
            fs::write(
                &entry,
                format!("project test fps 30/1 size 64x36\n[0,5) V1 {name}\n"),
            )
        };
        fs::create_dir(root.join("sequences"))?;
        fs::write(
            root.join("sequences/old.xml"),
            r##"<sequence background="#ffffff"/>"##,
        )?;
        write_master("old")?;
        let (_, initial) = ProjectInputs::load(root)?;
        let mut watch = ProjectWatch::start(&initial, Duration::from_millis(5))?;
        write_master("new")?;
        wait(&watch)?;
        let failure = ProjectInputs::load(root).err().expect("new detail missing");
        // Repair happens before the session delivers feedback to the worker.
        let leaf = root.join("sequences/new.xml");
        fs::write(&leaf, r##"<sequence background="#eeeeee"/>"##)?;
        watch.rejected(&failure);
        wait(&watch)?;
        let (_, candidate) = ProjectInputs::load(root)?;
        fs::write(&leaf, r##"<sequence background="#ffffff"/> "##)?;
        watch.accepted(&candidate);
        wait(&watch)?;
        Ok(())
    }
}
