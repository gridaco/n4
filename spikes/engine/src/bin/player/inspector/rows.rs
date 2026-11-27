use super::pane::{Axis, BG, BLUE, DIM, EDGE, GOLD, TEXT, ease_label, label};
use super::*;
use egui::{Align2, Color32, FontId, ScrollArea, Sense, Stroke, Ui, pos2, vec2};

const HEIGHT: f32 = 23.;

enum Row<'a> {
    Lane(&'a str),
    Layer(Vec<usize>, &'a Layer),
    Property(Vec<usize>, &'a Layer, usize),
}

fn layer_rows<'a>(
    layers: &'a [Layer],
    prefix: &mut Vec<usize>,
    v: &View,
    query: &str,
    rows: &mut Vec<Row<'a>>,
) {
    for (i, l) in layers.iter().enumerate() {
        prefix.push(i);
        let begin = rows.len();
        rows.push(Row::Layer(prefix.clone(), l));
        if v.opened.contains(prefix) || !query.is_empty() {
            let direct = l.name.to_lowercase().contains(query);
            for (j, t) in l.tracks.iter().enumerate() {
                if direct || t.name.to_lowercase().contains(query) {
                    rows.push(Row::Property(prefix.clone(), l, j));
                }
            }
            layer_rows(&l.children, prefix, v, query, rows);
            if !direct && rows.len() == begin + 1 {
                rows.pop();
            }
        }
        prefix.pop();
    }
}

pub(super) fn body(
    doc: &Inspection,
    v: &mut View,
    scope: &[usize],
    ui: &mut Ui,
    axis: Axis,
    cursor: u32,
    action: &mut Option<InspectorAction>,
) -> Option<Vec<usize>> {
    let area = ui.max_rect();
    let total = scope_range(doc, scope)[1] - axis.origin;
    let placements = scope_placements(doc, scope);
    let layers = scope_layers(doc, scope);
    let query = v.query.trim().to_lowercase();
    let mut rows = Vec::new();
    for lane in ["CAM", "V2", "V1", "A1"] {
        if placements.iter().any(|p| p.track == lane) {
            rows.push(Row::Lane(lane));
        }
    }
    layer_rows(layers, &mut Vec::new(), v, &query, &mut rows);
    let mut scroll = ScrollArea::vertical()
        .id_salt(("n4-inspector-rows", scope))
        .auto_shrink([false, false])
        .max_height(area.height());
    ui.spacing_mut().item_spacing.y = 0.;
    if v.reveal {
        if let Some(i) = rows.iter().position(|r| match r {
            Row::Property(path, _, t) => v.layer.as_ref() == Some(path) && v.track == Some(*t),
            Row::Layer(path, _) => v.layer.as_ref() == Some(path) && v.track.is_none(),
            _ => false,
        }) {
            scroll = scroll.vertical_scroll_offset(
                ((i as f32 - (area.height() * 0.35 / HEIGHT).round()).max(0.)) * HEIGHT,
            );
        }
        v.reveal = false;
    }
    let mut nav = None;
    scroll.show_rows(ui, HEIGHT, rows.len(), |ui, visible| {
        for idx in visible {
            let (row, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), HEIGHT), Sense::hover());
            let name_rect = Rect::from_min_max(row.min, pos2(axis.x, row.bottom()));
            let rail = Rect::from_min_max(pos2(axis.x, row.top()), pos2(axis.right, row.bottom()));
            if rail.width() <= 0. {
                continue;
            }
            if idx % 2 == 1 {
                ui.painter()
                    .rect_filled(row, 0., Color32::from_rgb(31, 33, 37));
            }
            match &rows[idx] {
                Row::Lane(name) => {
                    label(
                        ui,
                        name_rect.left_center() + vec2(10., 0.),
                        Align2::LEFT_CENTER,
                        *name,
                        11.,
                        lane_color(name),
                    );
                    label(
                        ui,
                        name_rect.left_center() + vec2(48., 0.),
                        Align2::LEFT_CENTER,
                        match *name {
                            "CAM" => "Camera cuts",
                            "V2" => "Overlay",
                            "A1" => "Audio",
                            _ => "Picture",
                        },
                        12.,
                        TEXT,
                    );
                    for (i, p) in placements
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| p.track == *name)
                    {
                        if p.local[1] <= axis.a || p.local[0] >= axis.b {
                            continue;
                        }
                        let rect = Rect::from_min_max(
                            pos2(axis.x(p.local[0]).max(rail.left()), rail.top() + 3.),
                            pos2(axis.x(p.local[1]).min(rail.right()), rail.bottom() - 2.),
                        );
                        if rect.width() <= 0. {
                            continue;
                        }
                        let active = v.clip == Some(i);
                        let tone = lane_color(name);
                        let paint = ui.painter().with_clip_rect(rect);
                        paint.rect_filled(
                            rect,
                            2.,
                            if active {
                                tone.gamma_multiply(0.57)
                            } else {
                                tone.gamma_multiply(0.29)
                            },
                        );
                        paint.rect_filled(
                            Rect::from_min_max(rect.min, rect.right_top() + vec2(0., 2.)),
                            0.,
                            tone,
                        );
                        paint.vline(rect.right() - 0.5, rect.y_range(), Stroke::new(1.0_f32, BG));
                        if rect.width() > 17. {
                            paint.text(
                                rect.left_center() + vec2(5., 0.5),
                                Align2::LEFT_CENTER,
                                &p.name,
                                FontId::proportional(11.5),
                                TEXT,
                            );
                        }
                        let hit = ui
                            .interact(rect, ui.id().with(("occurrence", &p.path)), Sense::click())
                            .on_hover_text(format!(
                                "{} · {}\nLocal [{}–{})   Film [{}–{})",
                                p.track, p.name, p.local[0], p.local[1], p.project[0], p.project[1]
                            ));
                        v.hit.insert(format!("placement:{}", p.path), rect.center());
                        if hit.clicked() {
                            v.layer = None;
                            v.track = None;
                            v.key = None;
                            v.clip = Some(i);
                            *action = Some(InspectorAction::Seek(if p.track == "CAM" {
                                p.project[0]
                            } else {
                                axis.origin
                                    + axis
                                        .frame(
                                            hit.interact_pointer_pos().unwrap_or(rect.center()).x,
                                        )
                                        .clamp(p.local[0], p.local[1] - 1)
                            }));
                        }
                        if hit.double_clicked() && p.track != "CAM" {
                            let mut next = scope.to_vec();
                            next.push(i);
                            nav = Some(next);
                        }
                    }
                }
                Row::Layer(path, l) => {
                    let selected = v.layer.as_ref() == Some(path) && v.track.is_none();
                    if selected {
                        ui.painter()
                            .rect_filled(name_rect, 0., Color32::from_rgb(54, 60, 71));
                    }
                    let open = v.opened.contains(path) || !query.is_empty();
                    let expandable = !l.tracks.is_empty() || !l.children.is_empty();
                    let x = name_rect.left()
                        + 10.
                        + ((path.len() - 1) as f32 * 13.).min(name_rect.width() * 0.40);
                    if expandable {
                        let c = pos2(x + 4., row.center().y);
                        let pts = if open {
                            vec![c + vec2(-3., -2.), c + vec2(3., -2.), c + vec2(0., 2.)]
                        } else {
                            vec![c + vec2(-2., -3.), c + vec2(2., 0.), c + vec2(-2., 3.)]
                        };
                        ui.painter()
                            .add(egui::Shape::convex_polygon(pts, DIM, Stroke::NONE));
                    }
                    let paint = ui.painter().with_clip_rect(name_rect.shrink2(vec2(3., 0.)));
                    paint.text(
                        pos2(x + 15., row.center().y),
                        Align2::LEFT_CENTER,
                        &l.name,
                        FontId::proportional(12.),
                        if selected {
                            TEXT
                        } else {
                            Color32::from_gray(204)
                        },
                    );
                    let hit = ui
                        .interact(name_rect, ui.id().with(("layer", path)), Sense::click())
                        .on_hover_text(format!("{} · {}", l.kind, l.name));
                    v.hit
                        .insert(format!("layer:{}", l.name), name_rect.center());
                    if hit.clicked() {
                        v.layer = Some(path.clone());
                        v.track = None;
                        v.key = None;
                        v.clip = None;
                        if expandable && !v.opened.remove(path) {
                            v.opened.insert(path.clone());
                        }
                    }
                    let first = axis.x(l.start).max(rail.left());
                    let last = axis.x(l.end.unwrap_or(total).min(total)).min(rail.right());
                    if first < last {
                        let span = Rect::from_min_max(
                            pos2(first, rail.center().y - 3.),
                            pos2(last, rail.center().y + 3.),
                        );
                        let c = if selected {
                            BLUE
                        } else if expandable {
                            Color32::from_rgb(96, 115, 149)
                        } else {
                            Color32::from_rgb(69, 78, 91)
                        };
                        ui.painter().rect_filled(span, 2., c);
                    }
                }
                Row::Property(path, l, j) => {
                    let t = &l.tracks[*j];
                    let selected = v.layer.as_ref() == Some(path) && v.track == Some(*j);
                    if selected {
                        ui.painter()
                            .rect_filled(row, 0., Color32::from_rgb(49, 55, 67));
                    }
                    let px = (15. + path.len() as f32 * 13.).min(name_rect.width() * 0.46);
                    ui.painter().with_clip_rect(name_rect).text(
                        name_rect.left_center() + vec2(px, 0.),
                        Align2::LEFT_CENTER,
                        &t.name,
                        FontId::proportional(11.5),
                        if selected { TEXT } else { DIM },
                    );
                    let name_hit = ui.interact(
                        name_rect,
                        ui.id().with(("prop-name", path, j)),
                        Sense::click(),
                    );
                    v.hit
                        .insert(format!("property:{}", t.name), name_rect.center());
                    if name_hit.clicked() {
                        v.layer = Some(path.clone());
                        v.track = Some(*j);
                        v.clip = None;
                        v.key = None;
                    }
                    let paint = ui.painter().with_clip_rect(rail);
                    let first = t.keys.first().map(|k| k.frame.saturating_add(l.start));
                    let last = t.keys.last().map(|k| k.frame.saturating_add(l.start));
                    if let Some((a, b)) = first.zip(last) {
                        paint.hline(
                            axis.x(a)..=axis.x(b),
                            rail.center().y,
                            Stroke::new(1.0_f32, EDGE),
                        );
                    }
                    let rail_hit =
                        ui.interact(rail, ui.id().with(("keys", path, j)), Sense::click());
                    let pointer = rail_hit.hover_pos().or(rail_hit.interact_pointer_pos());
                    let mut nearest = None;
                    let mut distance = 7.;
                    for k in &t.keys {
                        let local = l.start.saturating_add(k.frame);
                        if local < axis.a || local >= axis.b || local >= l.end.unwrap_or(total) {
                            continue;
                        }
                        // The first key is still fully pickable when exactly on the left boundary.
                        let x = axis.x(local).clamp(rail.left() + 3.5, rail.right() - 3.5);
                        let y = rail.center().y;
                        let point = pos2(x, y);
                        let radius = if selected && v.key == Some(k.frame) {
                            4.5
                        } else {
                            3.4
                        };
                        let color = if selected && v.key == Some(k.frame) {
                            GOLD
                        } else if selected {
                            BLUE
                        } else {
                            DIM
                        };
                        paint.add(egui::Shape::convex_polygon(
                            vec![
                                pos2(x, y - radius),
                                pos2(x + radius, y),
                                pos2(x, y + radius),
                                pos2(x - radius, y),
                            ],
                            color,
                            Stroke::NONE,
                        ));
                        if selected {
                            v.hit.insert(format!("key:{}", k.frame), point);
                        }
                        if let Some(p) = pointer {
                            let d = (p.x - x).abs();
                            if d <= distance {
                                distance = d;
                                nearest = Some(k);
                            }
                        }
                    }
                    if let Some(k) = nearest {
                        if rail_hit.clicked() {
                            v.layer = Some(path.clone());
                            v.track = Some(*j);
                            v.clip = None;
                            v.key = Some(k.frame);
                            *action = Some(InspectorAction::Seek(axis.origin + l.start + k.frame));
                        }
                        rail_hit.on_hover_text(format!(
                            "{}   ·   {}f local   ·   {}f film\n{}   ·   {}",
                            t.name,
                            k.frame,
                            axis.origin + l.start + k.frame,
                            f32::from(k.value),
                            ease_label(k.ease)
                        ));
                    }
                }
            }
            ui.painter()
                .vline(axis.x, row.y_range(), Stroke::new(1.0_f32, EDGE));
        }
    });
    if rows.is_empty() {
        label(
            ui,
            area.min + vec2(12., 10.),
            Align2::LEFT_TOP,
            if !query.is_empty() {
                "No matching layers"
            } else {
                "No layer properties"
            },
            12.,
            DIM,
        );
    }
    if let Some(f) = cursor
        .checked_sub(axis.origin)
        .filter(|f| *f >= axis.a && *f < axis.b)
    {
        let paint = ui.painter().with_clip_rect(area);
        paint.vline(axis.x(f), area.y_range(), Stroke::new(1.0_f32, GOLD));
    }
    nav
}

fn lane_color(name: &str) -> Color32 {
    match name {
        "CAM" => Color32::from_rgb(222, 175, 109),
        "A1" => Color32::from_rgb(102, 183, 158),
        "V2" => Color32::from_rgb(176, 150, 210),
        _ => Color32::from_rgb(126, 157, 216),
    }
}
