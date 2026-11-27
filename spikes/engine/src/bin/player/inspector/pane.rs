use super::*;
use egui::{
    Align2, Area, Button, Color32, FontId, Id, Order, RichText, Sense, Stroke, Ui, UiBuilder, pos2,
    vec2,
};
use n4_engine::graphics::Timing;

pub(super) const BG: Color32 = Color32::from_rgb(28, 30, 34);
pub(super) const BAND: Color32 = Color32::from_rgb(35, 38, 43);
pub(super) const EDGE: Color32 = Color32::from_rgb(56, 60, 68);
pub(super) const TEXT: Color32 = Color32::from_rgb(229, 232, 237);
pub(super) const DIM: Color32 = Color32::from_rgb(158, 166, 179);
pub(super) const GOLD: Color32 = Color32::from_rgb(255, 195, 108);
pub(super) const BLUE: Color32 = Color32::from_rgb(143, 171, 249);

#[derive(Clone, Copy)]
pub(super) struct Axis {
    pub x: f32,
    pub right: f32,
    pub a: u32,
    pub b: u32,
    pub origin: u32,
}
impl Axis {
    pub fn x(self, f: u32) -> f32 {
        self.x
            + (f as f64 - self.a as f64) as f32 / (self.b - self.a) as f32 * (self.right - self.x)
    }
    pub fn frame(self, x: f32) -> u32 {
        let offset = ((x - self.x) / (self.right - self.x).max(1.)).clamp(0., 1.);
        self.a + ((offset * (self.b - self.a) as f32).round() as u32).min(self.b - self.a - 1)
    }
}

pub(super) fn label(
    ui: &Ui,
    p: Pos2,
    align: Align2,
    value: impl ToString,
    size: f32,
    color: Color32,
) {
    ui.painter().text(
        p,
        align,
        value.to_string(),
        FontId::proportional(size),
        color,
    );
}
fn boxed_ui<R>(ui: &mut Ui, rect: Rect, render: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        ui.set_clip_rect(rect.intersect(ui.clip_rect()));
        ui.spacing_mut().item_spacing = vec2(4., 0.);
        render(ui)
    })
    .inner
}

pub(super) fn show(
    doc: &Inspection,
    v: &mut View,
    ctx: &Context,
    rect: Rect,
    cursor: u32,
) -> Option<InspectorAction> {
    let mut action = None;
    Area::new(Id::new("n4-inspector-panel"))
        .order(Order::Foreground)
        .fixed_pos(rect.min)
        .default_size(rect.size())
        .constrain(false)
        .movable(false)
        .fade_in(false)
        .sense(Sense::hover())
        .show(ctx, |ui| {
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());
            ui.set_clip_rect(rect);
            let p = ui.painter();
            p.rect_filled(rect, 0., BG);
            p.hline(rect.x_range(), rect.top(), Stroke::new(1.0_f32, EDGE));
            let scope = v.scope.clone();
            let [origin, end] = scope_range(doc, &scope);
            let total = (end - origin).max(1);
            let local = cursor.saturating_sub(origin).min(total - 1);
            let layers = scope_layers(doc, &scope);
            let header = Rect::from_min_size(rect.min, vec2(rect.width(), 31.));
            let foot = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 40.), rect.max);
            let scale = Rect::from_min_size(header.left_bottom(), vec2(rect.width(), 25.));
            let content = Rect::from_min_max(scale.left_bottom(), foot.right_top());
            let gutter = (rect.width() * 0.28).clamp(140., 300.);
            let ctrl_width = 161.;
            let mut switch_scope = None;
            let mut switch_to_frame = None;
            boxed_ui(
                ui,
                Rect::from_min_max(
                    header.min + vec2(8., 4.),
                    pos2(header.right() - ctrl_width - 5., header.bottom() - 2.),
                ),
                |ui| {
                    ui.horizontal(|ui| {
                        let mut crumbs = vec![("Timeline".to_owned(), 0)];
                        for n in 1..=scope.len() {
                            if let Some(p) = placement_at(&doc.placements, &scope[..n]) {
                                crumbs.push((p.name.clone(), n));
                            }
                        }
                        // Keep the nearest parent accessible on narrow inspector windows.
                        if ui.max_rect().width() < 420. && crumbs.len() > 3 {
                            crumbs.drain(1..crumbs.len() - 2);
                        }
                        for (i, (name, depth)) in crumbs.iter().enumerate() {
                            if i > 0 {
                                ui.label(RichText::new("›").color(DIM));
                            }
                            let current = *depth == scope.len();
                            let text = RichText::new(name)
                                .color(if current { TEXT } else { DIM })
                                .strong()
                                .size(12.);
                            let b = ui.add(Button::new(text).frame(false));
                            if b.clicked() && !current {
                                switch_scope = Some(scope[..*depth].to_vec());
                            }
                            v.hit.insert(format!("breadcrumb:{depth}"), b.rect.center());
                        }
                        if let Some(place) = placement_at(&doc.placements, &scope)
                            && let Some(definition) = &place.definition
                        {
                            let uses = doc.uses(definition);
                            if uses.len() > 1
                                && let Some(index) = uses.iter().position(|p| p.path == place.path)
                            {
                                let picker = egui::ComboBox::from_id_salt("n4-shared-uses")
                                    .width(93.)
                                    .selected_text(format!("Use {} of {}", index + 1, uses.len()))
                                    .show_ui(ui, |ui| {
                                        for (i, other) in uses.iter().enumerate() {
                                            let choice = ui.selectable_label(
                                                i == index,
                                                format!(
                                                    "Use {} · Film {}f",
                                                    i + 1,
                                                    other.project[0]
                                                ),
                                            );
                                            v.hit.insert(
                                                format!("use:{}", other.path),
                                                choice.rect.center(),
                                            );
                                            if choice.clicked() && i != index {
                                                let mut found = Vec::new();
                                                find_placements(
                                                    &doc.placements,
                                                    &other.path,
                                                    &mut Vec::new(),
                                                    &mut found,
                                                );
                                                switch_scope = found.pop();
                                                switch_to_frame = Some(
                                                    other.project[0]
                                                        + local.min(
                                                            other.project[1] - other.project[0] - 1,
                                                        ),
                                                );
                                            }
                                        }
                                    });
                                v.hit
                                    .insert("use-picker".into(), picker.response.rect.center());
                            }
                        }
                    });
                },
            );
            boxed_ui(
                ui,
                Rect::from_min_max(
                    pos2(header.right() - ctrl_width, header.top() + 4.),
                    header.max - vec2(4., 1.),
                ),
                |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.;
                        if button(ui, "‹", "Pan earlier", false).clicked() {
                            v.pan(-1, total);
                        }
                        if button(ui, "›", "Pan later", false).clicked() {
                            v.pan(1, total);
                        }
                        if button(ui, "−", "Zoom out around the playhead", false).clicked() {
                            v.zoom(2., local, total);
                        }
                        if button(ui, "+", "Zoom in around the playhead", false).clicked() {
                            v.zoom(0.5, local, total);
                        }
                        if button(ui, "Fit", "Fit this scope", false).clicked() {
                            v.range = None;
                        }
                        let close = button(ui, "×", "Close timeline inspector", false);
                        v.hit.insert("close".into(), close.rect.center());
                        if close.clicked() {
                            action = Some(InspectorAction::Close);
                        }
                    });
                },
            );
            let [a, b] = v.range.unwrap_or([0, total]);
            let axis = Axis {
                x: rect.left() + gutter,
                right: rect.right() - 13.,
                a,
                b,
                origin,
            };
            ui.painter().rect_filled(scale, 0., BAND);
            let sidebar = Rect::from_min_max(
                scale.min + vec2(10., 2.),
                pos2(axis.x - 5., scale.bottom() - 2.),
            );
            if layers.is_empty() {
                let placements = scope_placements(doc, &scope);
                if placements.is_empty() {
                    label(
                        ui,
                        sidebar.left_center(),
                        Align2::LEFT_CENTER,
                        "Tracks",
                        11.,
                        DIM,
                    );
                } else {
                    boxed_ui(ui, sidebar, |ui| {
                        egui::ComboBox::from_id_salt(("n4-placements", &scope))
                            .width(sidebar.width() - 8.)
                            .selected_text(RichText::new("Open a placement").size(11.))
                            .show_ui(ui, |ui| {
                                for (i, item) in placements.iter().enumerate() {
                                    if ui
                                        .selectable_label(
                                            false,
                                            format!(
                                                "{}   {}   ·   {}f",
                                                item.track, item.name, item.local[0]
                                            ),
                                        )
                                        .clicked()
                                    {
                                        v.clip = Some(i);
                                        if item.track == "CAM" {
                                            action = Some(InspectorAction::Seek(item.project[0]));
                                        } else {
                                            let mut next = scope.clone();
                                            next.push(i);
                                            switch_scope = Some(next);
                                        }
                                    }
                                }
                            });
                    });
                }
            } else {
                boxed_ui(ui, sidebar, |ui| {
                    let search = ui.add_sized(
                        sidebar.size(),
                        egui::TextEdit::singleline(&mut v.query)
                            .hint_text("Find layer or property")
                            .font(egui::TextStyle::Small),
                    );
                    v.hit.insert("search".into(), search.rect.center());
                });
            }
            let ruler =
                Rect::from_min_max(pos2(axis.x, scale.top()), pos2(axis.right, scale.bottom()));
            ruler_marks(ui, ruler, axis, cursor);
            let rp = ui.interact(ruler, ui.id().with("time-ruler"), Sense::click_and_drag());
            v.hit.insert("ruler".into(), ruler.center());
            let x = rp
                .interact_pointer_pos()
                .or_else(|| ctx.pointer_latest_pos())
                .map(|p| p.x);
            if let Some(x) = x {
                let film = origin + axis.frame(x);
                if rp.drag_started() {
                    action = Some(InspectorAction::Begin(film));
                } else if rp.drag_stopped() {
                    action = Some(InspectorAction::Land(film));
                } else if rp.dragged() {
                    action = Some(InspectorAction::Drag(film));
                } else if rp.clicked() {
                    action = Some(InspectorAction::Seek(film));
                }
            }
            boxed_ui(ui, content, |ui| {
                let nav = rows::body(doc, v, &scope, ui, axis, cursor, &mut action);
                if nav.is_some() {
                    switch_scope = nav;
                }
            });
            if let Some(next) = footer(doc, v, &scope, ui, foot, cursor, &mut action) {
                switch_scope = Some(next);
            }
            if let Some(target) = switch_scope {
                let [start, stop] = scope_range(doc, &target);
                if let Some(frame) = switch_to_frame {
                    action = Some(InspectorAction::Seek(frame));
                } else if cursor < start || cursor >= stop {
                    action = Some(InspectorAction::Seek(start));
                }
                v.enter(target);
            }
        });
    action
}

pub(super) fn button(ui: &mut Ui, text: &str, tip: &str, selected: bool) -> egui::Response {
    ui.add(
        Button::new(
            RichText::new(text)
                .size(12.)
                .color(if selected { GOLD } else { TEXT }),
        )
        .selected(selected)
        .min_size(vec2(19., 20.)),
    )
    .on_hover_text(tip)
}

fn ruler_marks(ui: &Ui, rect: Rect, axis: Axis, cursor: u32) {
    let span = axis.b - axis.a;
    let rough = (span as f32 / (rect.width() / 65.).max(1.)).max(1.);
    let power = 10f32.powf(rough.log10().floor());
    let step = ([1., 2., 5., 10.]
        .into_iter()
        .find(|x| x * power >= rough)
        .unwrap_or(10.)
        * power) as u32;
    let mut frame = axis.a.div_ceil(step).saturating_mul(step);
    while frame < axis.b {
        let x = axis.x(frame);
        ui.painter().vline(
            x,
            rect.bottom() - 5. ..=rect.bottom(),
            Stroke::new(1.0_f32, EDGE),
        );
        label(
            ui,
            pos2(x + 3., rect.top() + 3.),
            Align2::LEFT_TOP,
            format!("{frame}f"),
            10.,
            DIM,
        );
        let Some(next) = frame.checked_add(step) else {
            break;
        };
        frame = next;
    }
    if let Some(f) = cursor
        .checked_sub(axis.origin)
        .filter(|f| *f >= axis.a && *f < axis.b)
    {
        let x = axis.x(f);
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                pos2(x - 4., rect.bottom() - 5.),
                pos2(x + 4., rect.bottom() - 5.),
                pos2(x, rect.bottom()),
            ],
            GOLD,
            Stroke::NONE,
        ));
    }
}

fn footer(
    doc: &Inspection,
    v: &mut View,
    scope: &[usize],
    ui: &mut Ui,
    rect: Rect,
    cursor: u32,
    action: &mut Option<InspectorAction>,
) -> Option<Vec<usize>> {
    ui.painter().rect_filled(rect, 0., BAND);
    ui.painter()
        .hline(rect.x_range(), rect.top(), Stroke::new(1.0_f32, EDGE));
    let [origin, end] = scope_range(doc, scope);
    let local = cursor.checked_sub(origin).filter(|_| cursor < end);
    let layers = scope_layers(doc, scope);
    let item = v.layer.as_ref().and_then(|path| layer_at(layers, path));
    let track = item.and_then(|l| v.track.and_then(|i| l.tracks.get(i)));
    if let Some((l, t)) = item.zip(track) {
        let start = origin + l.start;
        let frame = cursor
            .checked_sub(start)
            .filter(|_| cursor < end.min(origin + l.end.unwrap_or(end - origin)));
        let value = frame.map_or_else(
            || "outside layer".into(),
            |f| format!("{:.3}", t.sample(f, 0.)),
        );
        let reference = v.key.or(frame);
        let ease = reference
            .and_then(|f| {
                let index = t.keys.iter().rposition(|k| k.frame <= f)?;
                if index + 1 < t.keys.len() {
                    Some(format!("Outgoing {}", ease_label(t.keys[index].ease)))
                } else {
                    index
                        .checked_sub(1)
                        .map(|i| format!("Arrives via {}", ease_label(t.keys[i].ease)))
                }
            })
            .unwrap_or_default();
        let title = format!("{}  /  {}", l.name, t.name);
        let chart_width = (rect.width() / 4.).min(198.);
        let chart = Rect::from_min_max(
            pos2(rect.right() - chart_width - 6., rect.top() + 4.),
            rect.max - vec2(6., 4.),
        );
        if !t.keys.is_empty() {
            curve(ui, chart, t, frame);
        }
        let info = ui.painter().with_clip_rect(Rect::from_min_max(
            rect.min,
            pos2((chart.left() - 70.).max(rect.left() + 50.), rect.bottom()),
        ));
        info.text(
            rect.min + vec2(10., 4.),
            Align2::LEFT_TOP,
            title,
            FontId::proportional(12.),
            TEXT,
        );
        let key_label = if let Some(f) = v.key {
            format!("Key {f}f  ·  Film {}f", start + f)
        } else {
            format!("Film {cursor}f")
        };
        info.text(
            rect.min + vec2(10., 22.),
            Align2::LEFT_TOP,
            if v.key.is_some() && v.key != frame {
                format!("{key_label}   ·   {ease}   ·   {value} at film {cursor}f")
            } else {
                format!("{key_label}   ·   {value}   ·   {ease}")
            },
            FontId::proportional(12.),
            DIM,
        );
        let buttons = Rect::from_min_size(
            pos2(chart.left() - 65., rect.center().y - 10.),
            vec2(60., 23.),
        );
        boxed_ui(ui, buttons, |ui| {
            ui.horizontal(|ui| {
                for (previous, symbol) in [(true, "‹"), (false, "›")] {
                    let step = button(
                        ui,
                        symbol,
                        if previous { "Previous key" } else { "Next key" },
                        false,
                    );
                    v.hit.insert(
                        if previous { "previous-key" } else { "next-key" }.into(),
                        step.rect.center(),
                    );
                    if step.clicked() {
                        let key = if previous {
                            t.keys.iter().rfind(|k| start + k.frame < cursor)
                        } else {
                            t.keys.iter().find(|k| start + k.frame > cursor)
                        };
                        if let Some(k) = key.filter(|k| start + k.frame < end) {
                            v.key = Some(k.frame);
                            *action = Some(InspectorAction::Seek(start + k.frame));
                        }
                    }
                }
            });
        });
    } else {
        let current = placement_at(&doc.placements, scope);
        let clip = v.clip.and_then(|i| scope_placements(doc, scope).get(i));
        let definition = clip
            .and_then(|p| p.definition.as_ref())
            .or_else(|| current.and_then(|p| p.definition.as_ref()));
        let use_count = definition.map(|id| doc.uses(id).len()).unwrap_or(0);
        let shared = if use_count > 1 {
            format!("   ·   Shared artwork · {use_count} uses")
        } else {
            String::new()
        };
        let (title, meta) = if let Some(l) = item {
            (
                l.name.clone(),
                format!(
                    "{}   ·   {} properties{}",
                    l.kind,
                    l.tracks.len(),
                    l.info
                        .first()
                        .map(|s| format!("   ·   {s}"))
                        .unwrap_or_default()
                ),
            )
        } else if let Some(p) = clip {
            let owner = if p.track == "CAM" {
                "Camera cut on this placement"
            } else {
                p.track.as_str()
            };
            (
                p.name.clone(),
                format!(
                    "{owner}   ·   Film [{}–{}){shared}",
                    p.project[0], p.project[1]
                ),
            )
        } else if let Some(p) = current {
            let default = definition
                .and_then(|id| doc.definitions.get(id))
                .and_then(|d| d.default_camera.as_deref())
                .map(|cam| format!("   ·   Default camera {cam}"))
                .unwrap_or_default();
            (
                p.name.clone(),
                format!(
                    "{}   ·   Film [{}–{}){shared}{default}",
                    p.track, p.project[0], p.project[1]
                ),
            )
        } else {
            (
                doc.name.clone(),
                format!(
                    "{} frames   ·   {:.3} fps",
                    doc.duration,
                    doc.fps.0 as f64 / doc.fps.1 as f64
                ),
            )
        };
        let info = ui.painter().with_clip_rect(Rect::from_min_max(
            rect.min,
            pos2(rect.right() - 151., rect.bottom()),
        ));
        info.text(
            rect.min + vec2(10., 4.),
            Align2::LEFT_TOP,
            title,
            FontId::proportional(12.),
            TEXT,
        );
        info.text(
            rect.min + vec2(10., 21.),
            Align2::LEFT_TOP,
            meta,
            FontId::proportional(12.),
            DIM,
        );
        if clip.is_some_and(|p| p.track != "CAM") {
            let r = Rect::from_min_size(pos2(rect.right() - 74., rect.top() + 8.), vec2(65., 23.));
            let open = ui.put(r, Button::new("Open ›"));
            v.hit.insert("open-placement".into(), open.rect.center());
            if open.clicked() {
                let mut next = scope.to_vec();
                next.push(v.clip.unwrap());
                return Some(next);
            }
        } else {
            label(
                ui,
                rect.right_top() + vec2(-10., 5.),
                Align2::RIGHT_TOP,
                format!("Film {cursor}f"),
                12.,
                TEXT,
            );
            if !scope.is_empty() {
                label(
                    ui,
                    rect.right_bottom() + vec2(-10., -4.),
                    Align2::RIGHT_BOTTOM,
                    local
                        .map(|f| format!("Local {f}f"))
                        .unwrap_or_else(|| "Outside placement".into()),
                    11.,
                    DIM,
                );
            }
        }
    }
    None
}

pub(super) fn ease_label(t: Timing) -> String {
    match t {
        Timing::Hold => "Hold".into(),
        Timing::Linear => "Linear".into(),
        Timing::Cubic(a) => {
            let [x1, y1, x2, y2] = a.map(f32::from);
            format!("Cubic ({x1:.2}, {y1:.2}, {x2:.2}, {y2:.2})")
        }
    }
}

fn curve(ui: &Ui, rect: Rect, track: &Track, frame: Option<u32>) {
    let (Some(first), Some(last)) = (track.keys.first(), track.keys.last()) else {
        return;
    };
    let start = first.frame;
    let length = last.frame.saturating_sub(start).max(1);
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(rect, 2., BG);
    let inner = rect.shrink(4.);
    // Sample the engine curve between authored keys, including fractional frames.
    let steps = (inner.width().ceil() as usize).clamp(8, 198);
    let mut values = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = f64::from(start) + f64::from(length) * i as f64 / steps as f64;
        values.push(track.sample(t.floor() as u32, t.fract()));
    }
    let (min, max) = values
        .iter()
        .copied()
        .chain(track.keys.iter().map(|k| f32::from(k.value)))
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), v| {
            (a.min(v), b.max(v))
        });
    let position = |phase: f32, value: f32| {
        pos2(
            inner.left() + inner.width() * phase,
            if max > min {
                inner.bottom() - inner.height() * (value - min) / (max - min)
            } else {
                inner.center().y
            },
        )
    };
    let line = values
        .iter()
        .enumerate()
        .map(|(i, v)| position(i as f32 / steps as f32, *v))
        .collect();
    p.add(egui::Shape::line(line, Stroke::new(1.5_f32, BLUE)));
    for k in &track.keys {
        p.circle_filled(
            position((k.frame - start) as f32 / length as f32, f32::from(k.value)),
            2.,
            TEXT,
        );
    }
    if let Some(f) = frame.filter(|f| *f >= start && *f <= last.frame) {
        let at = position((f - start) as f32 / length as f32, track.sample(f, 0.));
        p.vline(at.x, inner.y_range(), Stroke::new(1.0_f32, GOLD));
        p.circle_filled(at, 2.5, GOLD);
    }
}
