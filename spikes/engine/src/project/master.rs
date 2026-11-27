//! Read the time-first master used by the split authoring study. The master owns
//! placement intervals. Native shot and audio details own their original assets.
use super::{AuthoringDocument, Detail, DetailFile, FrameRate, GraphicsPlacement, Project, Title};
use crate::export::sample_boundary;
use crate::inspect::{Layout, Placement as InspectedPlacement};
use crate::project_watch::InputOwner;
use anyhow::{Context, Result, bail, ensure};
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    fs,
    path::Path,
};

fn id(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_')),
        "invalid definition ID {value:?}; use letters, digits, '-' or '_'"
    );
    Ok(())
}

fn pair(value: &str, separator: char) -> Result<(u32, u32)> {
    let (a, b) = value
        .split_once(separator)
        .context("expected a pair of integers")?;
    Ok((a.parse()?, b.parse()?))
}

fn header(line: &str) -> Result<(&str, Project)> {
    let words: Vec<_> = line.split_ascii_whitespace().collect();
    let ["project", entry, "fps", fps, "size", size] = words.as_slice() else {
        bail!("expected 'project <entry-id> fps <numerator>/<denominator> size <width>x<height>'");
    };
    id(entry)?;
    let (numerator, denominator) = pair(fps, '/')?;
    let (width, height) = pair(size, 'x')?;
    Ok((
        entry,
        Project {
            fps: FrameRate(numerator, denominator),
            size: [width, height],
            shots: vec![],
            overlays: vec![],
            title: Title::default(),
            audio: vec![],
            graphics: vec![],
            exposure: None,
        },
    ))
}

#[derive(Clone, Copy)]
struct MasterLine<'a> {
    number: usize,
    offset: usize,
    raw: &'a str,
    code: &'a str,
}

fn lines(source: &str) -> impl Iterator<Item = MasterLine<'_>> {
    source
        .split_inclusive('\n')
        .enumerate()
        .scan(0, |offset, (index, raw)| {
            let line = MasterLine {
                number: index + 1,
                offset: *offset,
                raw,
                code: raw.split('#').next().unwrap_or_default().trim_end(),
            };
            *offset += raw.len();
            Some(line)
        })
        .filter(|line| !line.code.is_empty())
}

fn take_v2_directive<'a>(
    lines: &mut std::iter::Peekable<impl Iterator<Item = MasterLine<'a>>>,
) -> Result<Option<(MasterLine<'a>, bool)>> {
    let Some(line) =
        lines.next_if(|line| line.code.split_ascii_whitespace().next() == Some("track"))
    else {
        return Ok(None);
    };
    let words: Vec<_> = line.code.split_ascii_whitespace().collect();
    let enabled = match words.as_slice() {
        ["track", "V2", "on"] if line.code.starts_with("track") => true,
        ["track", "V2", "off"] if line.code.starts_with("track") => false,
        _ => bail!(
            "line {}: expected a root 'track V2 on' or 'track V2 off' directive",
            line.number
        ),
    };
    Ok(Some((line, enabled)))
}

/// Change only the root track directive, preserving the original master text.
pub fn set_v2_enabled(bytes: &[u8], enabled: bool) -> Result<Vec<u8>> {
    let mut lines = lines(std::str::from_utf8(bytes)?).peekable();
    let header_line = lines.next().context("master is empty")?;
    header(header_line.code).with_context(|| format!("line {}", header_line.number))?;
    let current = take_v2_directive(&mut lines)?;
    let mut result = bytes.to_vec();
    match current {
        Some((line, previous)) if previous != enabled => {
            let end = line.offset + line.code.len();
            let previous_len = if previous { "on".len() } else { "off".len() };
            let value = if enabled { "on" } else { "off" };
            result.splice(end - previous_len..end, value.bytes());
        }
        None if !enabled => {
            let end = header_line.offset + header_line.raw.len();
            let directive = if header_line.raw.ends_with("\r\n") {
                "track V2 off\r\n"
            } else if header_line.raw.ends_with('\n') {
                "track V2 off\n"
            } else {
                "\ntrack V2 off"
            };
            result.splice(end..end, directive.bytes());
        }
        _ => {}
    }
    Ok(result)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Picture,
    Overlay,
    Audio,
    Sequence,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OverlayDetail {
    graphics: Vec<crate::graphics::Graphic>,
}

struct Placement<'a> {
    line: usize,
    depth: usize,
    start: u32,
    end: u32,
    track: &'a str,
    name: &'a str,
}

fn placement(number: usize, line: &str) -> Result<Placement<'_>> {
    let content = line.trim_start_matches(' ');
    let indent = line.len() - content.len();
    ensure!(
        indent.is_multiple_of(2) && !content.starts_with(char::is_whitespace),
        "expected two spaces per indentation level"
    );
    let words: Vec<_> = content.split_ascii_whitespace().collect();
    let [span, track, name] = words.as_slice() else {
        bail!("expected '[start,end) <track> <detail-id>'");
    };
    id(name)?;
    let (start, end) = pair(
        span.strip_prefix('[')
            .and_then(|s| s.strip_suffix(')'))
            .context("expected [start,end)")?,
        ',',
    )?;
    ensure!(start < end, "expected a positive placement duration");
    Ok(Placement {
        line: number,
        depth: indent / 2,
        start,
        end,
        track,
        name,
    })
}

/// Add master-owned values to the input of the ordinary, strict native serde
/// reader. Keeping those fields out of the detail prevents two timing authorities.
fn read_detail(
    directory: &Path,
    name: &str,
    kind: Detail,
    candidate: Option<(&str, &[u8])>,
    observe: &mut dyn FnMut(&Path),
) -> Result<DetailFile> {
    let stem = Path::new("sequences").join(name);
    let xml = stem.with_extension("xml");
    let json = stem.with_extension("json");
    observe(&directory.join(&xml));
    observe(&directory.join(&json));
    let path = if directory.join(&xml).try_exists()? {
        ensure!(
            !matches!(kind, Detail::Audio),
            "audio detail {name} requires {}",
            json.display()
        );
        ensure!(
            !directory.join(&json).try_exists()?,
            "ambiguous detail {name}: both {} and {} exist",
            xml.display(),
            json.display()
        );
        xml
    } else {
        ensure!(
            directory.join(&json).try_exists()?,
            "missing detail {name}: expected {} or {}",
            xml.display(),
            json.display()
        );
        json
    };
    let bytes = match candidate {
        Some((id, data)) if id == name => Cow::Borrowed(data),
        _ => Cow::Owned(
            fs::read(directory.join(&path))
                .with_context(|| format!("read detail {}", path.display()))?,
        ),
    };
    let authored = if path.extension().is_some_and(|ext| ext == "xml") {
        super::xml::decode(&bytes)
    } else {
        serde_json::from_slice(&bytes).map_err(anyhow::Error::from)
    }
    .with_context(|| format!("parse detail {}", path.display()))?;
    Ok(DetailFile {
        path,
        kind,
        authored,
    })
}

fn detail<T: serde::de::DeserializeOwned>(
    file: Option<&DetailFile>,
    placement_fields: &[(&str, u64)],
) -> Result<T> {
    let file = file.context("missing leaf definition")?;
    let path = &file.path;
    let mut value = file.authored.clone();
    let fields = value.as_object_mut().context("expected a detail object")?;
    for &(field, number) in placement_fields {
        ensure!(
            !fields.contains_key(field),
            "{}: {field} belongs to the master placement",
            path.display()
        );
        fields.insert(field.into(), number.into());
    }
    serde_json::from_value(value).with_context(|| format!("invalid detail {}", path.display()))
}

#[derive(Default)]
struct Scope {
    origin: u32,
    duration: Option<u32>,
    video_end: u32,
    audio_end: u32,
    overlay_end: Option<(u32, usize)>,
    overlays: Vec<GraphicsPlacement>,
    previous_start: u32,
    path: String,
    placements: Vec<InspectedPlacement>,
}

impl Scope {
    fn finish(mut self, project: &mut Project) -> Result<Vec<InspectedPlacement>> {
        ensure!(
            self.duration
                .is_none_or(|duration| self.video_end == duration),
            "nested V1 does not fill the parent placement starting at frame {}",
            self.origin
        );
        ensure!(
            self.audio_end <= self.video_end,
            "A1 extends beyond the picture"
        );
        if let Some((end, line)) = self.overlay_end {
            ensure!(
                end <= self.video_end,
                "line {line}: V2 extends beyond the picture"
            );
        }
        // Child scopes finish first, so the parent's V2 paints above every child lane.
        for (index, occurrence) in
            (project.overlays.len()..).zip(self.placements.iter_mut().filter(|p| p.track == "V2"))
        {
            occurrence.owner = Some(InputOwner::Overlay(index));
        }
        project.overlays.extend(self.overlays);
        Ok(self.placements)
    }

    fn place(
        &mut self,
        project: &mut Project,
        file: Option<&DetailFile>,
        row: &Placement,
        kind: Kind,
    ) -> Result<()> {
        ensure!(
            row.start >= self.previous_start,
            "expected time-first placement order in this sequence"
        );
        ensure!(
            self.duration.is_none_or(|duration| row.end <= duration),
            "placement exceeds parent duration"
        );
        self.previous_start = row.start;
        if kind == Kind::Overlay {
            if let Some((end, line)) = self.overlay_end {
                ensure!(
                    row.start >= end,
                    "overlapping V2 placements on lines {line} and {}",
                    row.line
                );
            }
            let artwork: OverlayDetail = detail(file, &[])?;
            self.overlays.push(GraphicsPlacement {
                detail: row.name.to_owned(),
                start: self
                    .origin
                    .checked_add(row.start)
                    .context("graphics placement start overflow")?,
                duration: row.end - row.start,
                enabled: true,
                graphics: artwork.graphics,
            });
            self.overlay_end = Some((row.end, row.line));
        } else if kind == Kind::Audio {
            ensure!(row.start >= self.audio_end, "overlapping A1 placements");
            let start = sample_boundary(
                project.fps,
                self.origin
                    .checked_add(row.start)
                    .context("audio start overflow")?,
            )?;
            let end = sample_boundary(
                project.fps,
                self.origin
                    .checked_add(row.end)
                    .context("audio end overflow")?,
            )?;
            project.audio.push(detail(
                file,
                &[("start_sample", start), ("samples", end - start)],
            )?);
            self.audio_end = row.end;
        } else {
            ensure!(
                row.start == self.video_end,
                "V1 must cover the picture without gaps or overlaps"
            );
            self.video_end = row.end;
            if kind == Kind::Picture {
                project.shots.push(detail(
                    file,
                    &[("duration", u64::from(row.end - row.start))],
                )?);
            }
        }
        Ok(())
    }
}

fn close_child(scopes: &mut Vec<Scope>, project: &mut Project) -> Result<()> {
    let children = scopes.pop().expect("child scope").finish(project)?;
    let parent = scopes
        .last_mut()
        .and_then(|scope| scope.placements.last_mut())
        .expect("each child scope follows its parent placement");
    parent.children = children;
    Ok(())
}

pub(super) fn read(
    directory: &Path,
    bytes: &[u8],
    candidate: Option<(&str, &[u8])>,
    observe: &mut dyn FnMut(&Path),
) -> Result<AuthoringDocument> {
    let mut lines = lines(std::str::from_utf8(bytes)?).peekable();
    let first = lines.next().context("master is empty")?;
    let (entry, mut project) =
        header(first.code).with_context(|| format!("line {}", first.number))?;
    let root_v2_enabled = take_v2_directive(&mut lines)?.is_none_or(|(_, enabled)| enabled);
    let rows = lines
        .map(|line| {
            placement(line.number, line.code).with_context(|| format!("line {}", line.number))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut kinds = HashMap::from([(entry, Kind::Sequence)]);
    let mut details = BTreeMap::new();
    let mut scopes = vec![Scope::default()];
    let mut i = 0;
    while i < rows.len() {
        let row = &rows[i];
        i += 1;
        let number = row.line;
        while scopes.len() > row.depth + 1 {
            close_child(&mut scopes, &mut project)
                .with_context(|| format!("before line {number}"))?;
        }
        ensure!(
            scopes.len() == row.depth + 1,
            "line {number}: unexpected indentation"
        );
        let mut cuts = Vec::new();
        while let Some(next) = rows.get(i)
            && next.depth == row.depth + 1
            && next.track == "CAM"
        {
            ensure!(
                row.track == "V1",
                "line {}: CAM must belong to a V1 picture",
                next.line
            );
            cuts.push(crate::camera::CameraCut {
                start: next.start,
                end: next.end,
                camera: next.name.to_owned(),
            });
            i += 1;
        }
        let has_children = rows.get(i).is_some_and(|next| next.depth > row.depth);
        ensure!(
            cuts.is_empty() || !has_children,
            "line {number}: a V1 placement cannot contain both camera cuts and a nested edit"
        );
        let kind = match (row.track, has_children) {
            ("V1", true) => Kind::Sequence,
            ("V1", false) => Kind::Picture,
            ("V2", false) => Kind::Overlay,
            ("A1", false) => Kind::Audio,
            ("A1" | "V2", true) => bail!("line {number}: only V1 can declare a nested sequence"),
            _ => bail!(
                "line {number}: unsupported track {}; expected V1, V2 or A1",
                row.track
            ),
        };
        if let Some(old) = kinds.insert(row.name, kind) {
            ensure!(
                old == kind && kind != Kind::Sequence,
                "line {number}: conflicting definition for {}",
                row.name
            );
        }
        let definition = match kind {
            Kind::Audio => Some(Detail::Audio),
            Kind::Picture => Some(Detail::Shot(project.shots.len())),
            Kind::Overlay => Some(Detail::Overlay),
            Kind::Sequence => None,
        };
        if let Some(kind) = definition
            && !details.contains_key(row.name)
        {
            let file = read_detail(directory, row.name, kind, candidate, observe)
                .with_context(|| format!("line {number}"))?;
            details.insert(row.name.to_owned(), file);
        }
        let owner = scopes.last_mut().unwrap();
        owner
            .place(&mut project, details.get(row.name), row, kind)
            .with_context(|| format!("line {number}"))?;
        let mut occurrence = InspectedPlacement::new(
            &owner.path,
            owner.origin,
            row.name,
            row.track,
            [row.start, row.end],
            (kind != Kind::Sequence).then(|| row.name.to_owned()),
        )?;
        occurrence.owner = match kind {
            Kind::Picture => Some(InputOwner::Shot(project.shots.len() - 1)),
            Kind::Audio => Some(InputOwner::Audio(project.audio.len() - 1)),
            Kind::Overlay | Kind::Sequence => None,
        };
        if kind == Kind::Picture {
            let shot = project.shots.last_mut().expect("picture just placed");
            ensure!(
                shot.camera_cuts.is_empty(),
                "line {number}: camera cuts belong to the master placement"
            );
            shot.camera_cuts = cuts;
            shot.validate_cameras()
                .with_context(|| format!("line {number}: {}", row.name))?;
            occurrence.set_cuts(&shot.camera_cuts)?;
        }
        owner.placements.push(occurrence);
        if kind == Kind::Sequence {
            let parent = owner.placements.last().expect("sequence just placed");
            let path = parent.path.clone();
            let origin = parent.project[0];
            scopes.push(Scope {
                path,
                origin,
                duration: Some(row.end - row.start),
                ..Scope::default()
            });
        }
    }
    while scopes.len() > 1 {
        close_child(&mut scopes, &mut project)?;
    }
    let mut root = scopes.pop().unwrap();
    if !root_v2_enabled {
        for overlay in &mut root.overlays {
            overlay.enabled = false;
        }
        for placement in root.placements.iter_mut().filter(|p| p.track == "V2") {
            placement.enabled = false;
        }
    }
    let placements = root.finish(&mut project)?;
    if let Some((name, _)) = candidate {
        ensure!(
            details.contains_key(name),
            "detail {name:?} is not placed in the master"
        );
    }
    Ok(AuthoringDocument {
        project,
        details,
        layout: Layout::Master {
            name: entry.into(),
            placements,
            root_v2_enabled,
        },
    })
}
