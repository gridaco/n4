//! Read-only timeline state. The only movie operations emitted by the inspector are seeks.
use anyhow::{Context as _, Result, bail, ensure};
use egui::{Context, Pos2, Rect};
use n4_engine::inspect::{Inspection, Layer, Placement, Track};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

mod pane;
mod rows;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectorAction {
    Seek(u32),
    Begin(u32),
    Drag(u32),
    Land(u32),
    Close,
}

pub struct Inspector {
    model: Inspection,
    view: View,
}

#[derive(Default)]
struct View {
    scope: Vec<usize>,
    opened: BTreeSet<Vec<usize>>,
    layer: Option<Vec<usize>>,
    track: Option<usize>,
    key: Option<u32>,
    clip: Option<usize>,
    /// Detail-relative half-open visible frame range. None means fit the current scope.
    range: Option<[u32; 2]>,
    history: BTreeMap<Vec<usize>, Option<[u32; 2]>>,
    query: String,
    reveal: bool,
    hit: BTreeMap<String, Pos2>,
}

impl Inspector {
    pub fn new(model: Inspection) -> Self {
        Self {
            model,
            view: View::default(),
        }
    }

    /// Opens a unique named placement or its complete occurrence path. Layer names may
    /// likewise use a slash-separated chain to distinguish same-named descendants.
    pub fn focus(
        &mut self,
        occurrence: &str,
        layer: Option<&str>,
        property: Option<&str>,
    ) -> Result<()> {
        let mut matches = Vec::new();
        find_placements(
            &self.model.placements,
            occurrence,
            &mut Vec::new(),
            &mut matches,
        );
        let scope = match matches.as_slice() {
            [] if occurrence.is_empty() || occurrence == "master" => Vec::new(),
            [path] => path.clone(),
            [] => bail!("unknown placement {occurrence:?}"),
            _ => bail!("multiple uses of {occurrence:?}; supply the complete occurrence path"),
        };
        let mut view = View {
            scope,
            ..Default::default()
        };
        ensure!(
            property.is_none() || layer.is_some(),
            "a property requires an owning layer"
        );
        if let Some(name) = layer {
            let layers = scope_layers(&self.model, &view.scope);
            let mut found = Vec::new();
            find_layers(layers, name, "", &mut Vec::new(), &mut found);
            let path = match found.as_slice() {
                [path] => path.clone(),
                [] => bail!("unknown layer {name:?}"),
                _ => bail!("multiple layers named {name:?}; supply their hierarchy"),
            };
            for depth in 1..=path.len() {
                view.opened.insert(path[..depth].to_vec());
            }
            let item = layer_at(layers, &path).context("invalid layer address")?;
            view.track = property
                .map(|name| {
                    item.tracks
                        .iter()
                        .position(|t| t.name == name)
                        .with_context(|| format!("unknown property {name:?} on {}", item.name))
                })
                .transpose()?;
            view.layer = Some(path);
            view.reveal = true;
        }
        self.view = view;
        Ok(())
    }

    /// Restore selection by the authored placement, layer path and track name.
    /// Reordering unrelated layers must not silently select a different object.
    pub fn reopen(&mut self, model: Inspection) {
        let prior = &self.view;
        let scope = placement_at(&self.model.placements, &prior.scope)
            .map_or("master", |place| place.path.as_str());
        let mut layers = scope_layers(&self.model, &prior.scope);
        let mut names = Vec::new();
        let mut selected = None;
        for &i in prior.layer.iter().flatten() {
            let Some(layer) = layers.get(i) else { break };
            names.push(layer.name.as_str());
            selected = Some(layer);
            layers = &layer.children;
        }
        let path = (!names.is_empty()).then(|| names.join("/"));
        let property = selected
            .and_then(|l| prior.track.and_then(|i| l.tracks.get(i)))
            .map(|t| t.name.as_str());
        let mut next = Self::new(model);
        if next.focus(scope, path.as_deref(), property).is_err()
            && next.focus(scope, path.as_deref(), None).is_err()
        {
            let _ = next.focus(scope, None, None);
        }
        if let (Some(indices), Some(track), Some(frame)) =
            (&next.view.layer, next.view.track, prior.key)
        {
            let selected = layer_at(scope_layers(&next.model, &next.view.scope), indices)
                .and_then(|l| l.tracks.get(track));
            if selected.is_some_and(|t| t.keys.iter().any(|k| k.frame == frame)) {
                next.view.key = Some(frame);
            }
        }
        *self = next;
    }

    pub fn scope_start(&self) -> u32 {
        scope_range(&self.model, &self.view.scope)[0]
    }

    pub fn hit_point(&self, target: &str) -> Option<Pos2> {
        self.view.hit.get(target).copied()
    }

    pub fn snapshot(&self, cursor: u32) -> Value {
        let v = &self.view;
        let place = placement_at(&self.model.placements, &v.scope);
        let [origin, end] = scope_range(&self.model, &v.scope);
        let layer = v
            .layer
            .as_ref()
            .and_then(|p| layer_at(scope_layers(&self.model, &v.scope), p));
        let track = layer.and_then(|l| v.track.and_then(|i| l.tracks.get(i)));
        let layer_frame = layer.and_then(|l| {
            let detail_frame = cursor.checked_sub(origin)?;
            (cursor < end && l.end.is_none_or(|e| detail_frame < e)).then_some(())?;
            detail_frame.checked_sub(l.start)
        });
        let keyframes = track.zip(layer).map(|(t,l)| t.keys.iter().map(|k|
            json!({"frame":k.frame, "project_frame":origin + l.start + k.frame, "value":f32::from(k.value), "ease":k.ease})
        ).collect::<Vec<_>>());
        json!({
            "scope":place.map(|p|p.path.as_str()), "project_frame":cursor,
            "scope_local_frame":cursor.checked_sub(origin).filter(|_|cursor<end),
            "layer":layer.map(|l|l.name.as_str()), "property":track.map(|t|t.name.as_str()),
            "layer_local_frame":layer_frame, "selected_key":v.key,
            "current_value":track.zip(layer_frame).map(|(t,f)| t.sample(f,0.)),
            "active_key":track.zip(layer_frame).and_then(|(t,f)| t.keys.iter().rev().find(|k|k.frame<=f).map(|k|k.frame)),
            "keys":keyframes, "visible_local_range":v.range.unwrap_or([0,end-origin]),
            "selected_placement":v.clip.and_then(|i|scope_placements(&self.model,&v.scope).get(i)).map(|p|p.path.as_str()),
        })
    }

    pub fn show(&mut self, ctx: &Context, rect: Rect, cursor: u32) -> Option<InspectorAction> {
        self.view.hit.clear();
        pane::show(&self.model, &mut self.view, ctx, rect, cursor)
    }
}

impl View {
    fn enter(&mut self, scope: Vec<usize>) {
        let previous = std::mem::replace(&mut self.scope, scope);
        self.history.insert(previous, self.range);
        self.range = self.history.get(&self.scope).copied().flatten();
        self.opened.clear();
        self.query.clear();
        self.layer = None;
        self.track = None;
        self.key = None;
        self.clip = None;
        self.reveal = false;
    }

    fn zoom(&mut self, factor: f32, anchor: u32, total: u32) {
        let [a, b] = self.range.unwrap_or([0, total]);
        let width = b - a;
        let new_width = ((width as f32 * factor).round() as u32).clamp(2.min(total), total);
        let fraction = f64::from(anchor.clamp(a, b) - a) / f64::from(width);
        let start = ((f64::from(anchor.clamp(a, b)) - fraction * f64::from(new_width)).round()
            as u32)
            .min(total - new_width);
        self.range = (new_width < total).then_some([start, start + new_width]);
    }

    fn pan(&mut self, direction: i32, total: u32) {
        if let Some([a, b]) = self.range {
            let width = b - a;
            let start = (i64::from(a) + i64::from(direction) * i64::from((width / 3).max(1)))
                .clamp(0, i64::from(total - width)) as u32;
            self.range = Some([start, start + width]);
        }
    }
}

fn placement_at<'a>(mut entries: &'a [Placement], path: &[usize]) -> Option<&'a Placement> {
    let mut value = None;
    for &i in path {
        let p = entries.get(i)?;
        value = Some(p);
        entries = &p.children;
    }
    value
}

fn scope_placements<'a>(doc: &'a Inspection, path: &[usize]) -> &'a [Placement] {
    placement_at(&doc.placements, path).map_or(doc.placements.as_slice(), |p| &p.children)
}

fn scope_range(doc: &Inspection, path: &[usize]) -> [u32; 2] {
    placement_at(&doc.placements, path).map_or([0, doc.duration], |p| p.project)
}

fn scope_layers<'a>(doc: &'a Inspection, path: &[usize]) -> &'a [Layer] {
    placement_at(&doc.placements, path)
        .and_then(|p| p.definition.as_ref())
        .and_then(|id| doc.definitions.get(id))
        .map_or(&[], |d| d.layers.as_slice())
}

fn layer_at<'a>(mut layers: &'a [Layer], path: &[usize]) -> Option<&'a Layer> {
    let mut value = None;
    for &i in path {
        let l = layers.get(i)?;
        value = Some(l);
        layers = &l.children;
    }
    value
}

fn find_placements(
    nodes: &[Placement],
    query: &str,
    prefix: &mut Vec<usize>,
    found: &mut Vec<Vec<usize>>,
) {
    for (i, p) in nodes.iter().enumerate() {
        prefix.push(i);
        if p.name == query || p.path == query {
            found.push(prefix.clone());
        }
        find_placements(&p.children, query, prefix, found);
        prefix.pop();
    }
}

fn find_layers(
    nodes: &[Layer],
    query: &str,
    parent: &str,
    prefix: &mut Vec<usize>,
    found: &mut Vec<Vec<usize>>,
) {
    for (i, l) in nodes.iter().enumerate() {
        prefix.push(i);
        let path = if parent.is_empty() {
            l.name.clone()
        } else {
            format!("{parent}/{}", l.name)
        };
        if l.name == query || path == query {
            found.push(prefix.clone());
        }
        find_layers(&l.children, query, &path, prefix, found);
        prefix.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, RawInput, pos2, vec2};
    use n4_engine::AuthoringDocument;
    use std::path::Path;

    struct Harness {
        pane: Inspector,
        ctx: Context,
        tick: u32,
        cursor: u32,
    }

    impl Harness {
        fn open(project: &str) -> Result<Self> {
            let base = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join(project);
            let doc = AuthoringDocument::load(&base, None)?;
            Ok(Self {
                pane: Inspector::new(doc.inspection()?),
                ctx: Default::default(),
                tick: 0,
                cursor: 0,
            })
        }
        fn frame(&mut self, events: Vec<Event>) -> Option<InspectorAction> {
            let screen = Rect::from_min_size(Pos2::ZERO, vec2(1080., 740.));
            let pane = Rect::from_min_size(pos2(0., 300.), vec2(1080., 280.));
            let input = RawInput {
                screen_rect: Some(screen),
                events,
                time: Some(f64::from(self.tick) / 60.),
                ..Default::default()
            };
            self.tick += 1;
            let mut act = None;
            let _ = self
                .ctx
                .run(input, |ctx| act = self.pane.show(ctx, pane, self.cursor));
            if let Some(InspectorAction::Seek(n)) = act {
                self.cursor = n;
            }
            act
        }
        fn click(&mut self, target: &str) -> Result<Option<InspectorAction>> {
            for _ in 0..3 {
                self.frame(vec![]);
            }
            let p = self
                .pane
                .hit_point(target)
                .with_context(|| format!("unreachable hit {target}"))?;
            ensure!(
                p.y >= 300. && p.y <= 580.,
                "target falls outside the pane: {p:?}"
            );
            let button = |pressed| Event::PointerButton {
                pos: p,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            self.frame(vec![Event::PointerMoved(p), button(true)]);
            Ok(self.frame(vec![button(false)]))
        }
    }

    #[test]
    fn a_real_key_click_seeks_the_composition_on_nested_layer_time() -> Result<()> {
        let mut h = Harness::open("examples/signal-atlas")?;
        h.pane
            .focus("sounding", Some("Signal lamp"), Some("opacity"))?;
        h.cursor = h.pane.scope_start();
        assert_eq!(h.click("key:4")?, Some(InspectorAction::Seek(76)));
        let state = h.pane.snapshot(76);
        assert_eq!(state["current_value"], 1.);
        assert_eq!(state["layer_local_frame"], 4);
        assert_eq!(state["selected_key"], 4);
        Ok(())
    }

    #[test]
    fn nested_ruler_maps_forward_reverse_and_release_to_the_film() -> Result<()> {
        let mut h = Harness::open("examples/signal-atlas")?;
        h.pane
            .focus("sounding", Some("Signal lamp"), Some("opacity"))?;
        h.cursor = h.pane.scope_start();
        for _ in 0..3 {
            h.frame(vec![]);
        }

        // On the 64-frame detail ruler the card's two keys sit at local 8 and 12.
        // Use the actual displayed diamonds as independent screen-coordinate witnesses.
        let y = h.pane.hit_point("ruler").context("missing ruler")?.y;
        let x0 = h.pane.hit_point("key:0").context("missing first key")?.x;
        let x4 = h.pane.hit_point("key:4").context("missing second key")?.x;
        let first = pos2(x0, y);
        let second = pos2(x4, y);
        let far = pos2(x0 + 3. * (x4 - x0), y);
        let button = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        assert_eq!(
            h.frame(vec![Event::PointerMoved(first), button(first, true)]),
            None
        );
        assert_eq!(
            h.frame(vec![Event::PointerMoved(second)]),
            Some(InspectorAction::Begin(76))
        );
        assert_eq!(
            h.frame(vec![Event::PointerMoved(far)]),
            Some(InspectorAction::Drag(84))
        );
        assert_eq!(
            h.frame(vec![Event::PointerMoved(first)]),
            Some(InspectorAction::Drag(72))
        );
        assert_eq!(
            h.frame(vec![Event::PointerMoved(second), button(second, false)]),
            Some(InspectorAction::Land(76))
        );
        assert_eq!(h.frame(vec![]), None);
        Ok(())
    }

    #[test]
    fn explore_from_master_find_a_layer_and_step_between_its_keys() -> Result<()> {
        let mut h = Harness::open("examples/signal-atlas")?;
        h.click("placement:V1:analysis@64")?;
        h.click("open-placement")?;
        h.click("placement:V1:analysis@64/V1:sounding@0")?;
        h.click("open-placement")?;
        assert_eq!(h.pane.scope_start(), 64);
        h.click("search")?;
        h.frame(vec![Event::Text("Signal lamp".into())]);
        h.click("property:opacity")?;
        assert_eq!(h.click("key:4")?, Some(InspectorAction::Seek(76)));
        assert_eq!(h.click("previous-key")?, Some(InspectorAction::Seek(72)));
        assert_eq!(h.click("next-key")?, Some(InspectorAction::Seek(76)));
        h.click("breadcrumb:0")?;
        assert!(h.pane.snapshot(76)["scope"].is_null());
        assert_eq!(h.cursor, 76);
        Ok(())
    }

    #[test]
    fn a_camera_cut_is_selected_only_on_the_shared_placement_that_owns_it() -> Result<()> {
        let mut h = Harness::open("examples/signal-atlas")?;
        assert!(h.pane.focus("survey", None, None).is_err());
        h.pane.focus("V1:survey@128", None, None)?;
        h.cursor = h.pane.scope_start();
        let cut = "V1:survey@128/CAM:detail@12";
        assert_eq!(
            h.click(&format!("placement:{cut}"))?,
            Some(InspectorAction::Seek(140))
        );
        assert_eq!(h.pane.snapshot(140)["selected_placement"], cut);
        h.click("use-picker")?;
        assert_eq!(h.click("use:V1:survey@0")?, Some(InspectorAction::Seek(12)));
        assert_eq!(h.pane.snapshot(12)["scope"], "V1:survey@0");
        h.frame(vec![]);
        assert!(h.pane.hit_point(&format!("placement:{cut}")).is_none());
        Ok(())
    }
}
