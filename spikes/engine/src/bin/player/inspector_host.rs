//! SDL input and presentation for the optional, read-only inspector.
use super::{
    Coordinates,
    egui_canvas::Painter,
    inspector::{Inspector, InspectorAction},
};
use anyhow::Result;
use egui::{Key, Modifiers, PointerButton, Pos2, RawInput, Rect, ViewportId, pos2, vec2};
use sdl2::{
    event::{Event, WindowEvent},
    keyboard::{Keycode, Mod},
    mouse::{Cursor, MouseButton, MouseWheelDirection, SystemCursor},
    render::{TextureCreator, WindowCanvas},
    video::WindowContext,
};
use std::time::Instant;

pub struct InspectorHost<'a> {
    pub pane: Inspector,
    context: egui::Context,
    painter: Painter<'a>,
    input: RawInput,
    started: Instant,
    repaint: Option<Instant>,
    pointer: Option<Pos2>,
    captured: bool,
    text_focus: bool,
    composing: bool,
    cursor: Option<(SystemCursor, Cursor)>,
}

impl<'a> InspectorHost<'a> {
    pub fn new(creator: &'a TextureCreator<WindowContext>, pane: Inspector) -> Self {
        Self {
            pane,
            context: egui::Context::default(),
            painter: Painter::new(creator),
            input: RawInput::default(),
            started: Instant::now(),
            repaint: None,
            pointer: None,
            captured: false,
            text_focus: false,
            composing: false,
            cursor: None,
        }
    }
    pub fn height(h: u32) -> u32 {
        ((h as f32 * 0.40).round() as u32).min(h.saturating_sub(240))
    }
    fn area(coords: Coordinates) -> Rect {
        let (w, h) = coords.points;
        let bottom = h.saturating_sub(super::timeline::STRIP_HEIGHT) as f32;
        Rect::from_min_max(
            pos2(0., bottom - Self::height(h) as f32),
            pos2(w as f32, bottom),
        )
    }
    pub fn pending(&self) -> bool {
        !self.input.events.is_empty() || self.repaint.is_some_and(|t| t <= Instant::now())
    }
    fn position(coords: Coordinates, x: i32, y: i32) -> Pos2 {
        let (x, y) = coords.point(x, y);
        pos2(x as f32, y as f32)
    }
    /// The legacy strip owns its drags through release even when crossing this panel.
    pub fn event(
        &mut self,
        event: &Event,
        coords: Coordinates,
        legacy_drag: bool,
        video: &sdl2::VideoSubsystem,
    ) -> bool {
        let area = Self::area(coords);
        match *event {
            Event::MouseMotion { x, y, .. } => {
                let p = Self::position(coords, x, y);
                if self.captured || (area.contains(p) && !legacy_drag) {
                    self.input.events.push(egui::Event::PointerMoved(p));
                    self.pointer = Some(p);
                    return true;
                }
                if self.pointer.take().is_some() {
                    self.input.events.push(egui::Event::PointerGone);
                    self.set_cursor(SystemCursor::Arrow);
                }
            }
            Event::MouseButtonDown {
                mouse_btn, x, y, ..
            }
            | Event::MouseButtonUp {
                mouse_btn, x, y, ..
            } => {
                let p = Self::position(coords, x, y);
                let pressed = matches!(event, Event::MouseButtonDown { .. });
                if self.captured || (!legacy_drag && area.contains(p)) {
                    let button = match mouse_btn {
                        MouseButton::Left => PointerButton::Primary,
                        MouseButton::Right => PointerButton::Secondary,
                        MouseButton::Middle => PointerButton::Middle,
                        _ => return false,
                    };
                    self.input.events.push(egui::Event::PointerMoved(p));
                    self.input.events.push(egui::Event::PointerButton {
                        pos: p,
                        button,
                        pressed,
                        modifiers: self.input.modifiers,
                    });
                    self.pointer = Some(p);
                    self.captured = pressed;
                    return true;
                }
                if pressed && self.text_focus {
                    self.blur(video);
                }
            }
            Event::MouseWheel {
                precise_x,
                precise_y,
                direction,
                ..
            } if self.pointer.is_some() => {
                let sign = if direction == MouseWheelDirection::Flipped {
                    -1.
                } else {
                    1.
                };
                self.input.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: vec2(precise_x, precise_y) * sign,
                    modifiers: self.input.modifiers,
                });
                return true;
            }
            Event::KeyDown {
                keycode,
                keymod,
                repeat,
                ..
            }
            | Event::KeyUp {
                keycode,
                keymod,
                repeat,
                ..
            } => {
                if matches!(keycode, Some(Keycode::F5))
                    || (keycode == Some(Keycode::R)
                        && keymod.intersects(Mod::LGUIMOD | Mod::RGUIMOD))
                {
                    return false;
                }
                self.input.modifiers = modifiers(keymod);
                let pressed = matches!(event, Event::KeyDown { .. });
                if self.text_focus && self.input.modifiers.command && pressed {
                    match keycode {
                        Some(Keycode::V) => {
                            if let Ok(s) = video.clipboard().clipboard_text() {
                                self.input.events.push(egui::Event::Paste(s));
                            }
                            return true;
                        }
                        Some(Keycode::C) => {
                            self.input.events.push(egui::Event::Copy);
                            return true;
                        }
                        Some(Keycode::X) => {
                            self.input.events.push(egui::Event::Cut);
                            return true;
                        }
                        _ => {}
                    }
                }
                let key = match keycode {
                    Some(Keycode::Tab) => Key::Tab,
                    Some(Keycode::Return) => Key::Enter,
                    Some(Keycode::Up) => Key::ArrowUp,
                    Some(Keycode::Down) => Key::ArrowDown,
                    Some(key) if self.text_focus => match key {
                        Keycode::Backspace => Key::Backspace,
                        Keycode::Delete => Key::Delete,
                        Keycode::Left => Key::ArrowLeft,
                        Keycode::Right => Key::ArrowRight,
                        Keycode::Escape => Key::Escape,
                        Keycode::Home => Key::Home,
                        Keycode::End => Key::End,
                        Keycode::A => Key::A,
                        Keycode::Z => Key::Z,
                        Keycode::Y => Key::Y,
                        Keycode::Q if self.input.modifiers.command => return false,
                        _ => return true,
                    },
                    _ => return false,
                };
                self.input.events.push(egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed,
                    repeat,
                    modifiers: self.input.modifiers,
                });
                return true;
            }
            Event::TextEditing { ref text, .. } if self.text_focus => {
                if !self.composing {
                    self.input
                        .events
                        .push(egui::Event::Ime(egui::ImeEvent::Enabled));
                }
                self.composing = true;
                self.input
                    .events
                    .push(egui::Event::Ime(egui::ImeEvent::Preedit(text.clone())));
                return true;
            }
            Event::TextInput { ref text, .. } if self.text_focus => {
                self.input.events.push(if self.composing {
                    egui::Event::Ime(egui::ImeEvent::Commit(text.clone()))
                } else {
                    egui::Event::Text(text.clone())
                });
                if self.composing {
                    self.input
                        .events
                        .push(egui::Event::Ime(egui::ImeEvent::Disabled));
                    self.composing = false;
                }
                return true;
            }
            Event::Window {
                win_event: WindowEvent::FocusLost,
                ..
            } => {
                self.blur(video);
                self.input.focused = false;
                self.input.events.push(egui::Event::WindowFocused(false));
                self.input.events.push(egui::Event::PointerGone);
                self.pointer = None;
                self.captured = false;
            }
            Event::Window {
                win_event: WindowEvent::FocusGained,
                ..
            } => {
                self.input.focused = true;
                self.input.events.push(egui::Event::WindowFocused(true));
            }
            _ => {}
        }
        false
    }

    fn blur(&mut self, video: &sdl2::VideoSubsystem) {
        if self.text_focus {
            video.text_input().stop();
        }
        self.context.memory_mut(|memory| {
            if let Some(id) = memory.focused() {
                memory.surrender_focus(id);
            }
        });
        self.text_focus = false;
        self.composing = false;
    }
    pub fn suspend(&mut self, video: &sdl2::VideoSubsystem) {
        self.blur(video);
        self.captured = false;
        self.pointer = None;
        self.input.events.clear();
        self.input.events.push(egui::Event::PointerGone);
        self.set_cursor(SystemCursor::Arrow);
    }

    fn set_cursor(&mut self, icon: SystemCursor) {
        if self.cursor.as_ref().is_none_or(|(old, _)| *old != icon)
            && let Ok(cursor) = Cursor::from_system(icon)
        {
            cursor.set();
            self.cursor = Some((icon, cursor));
        }
    }
    pub fn draw(
        &mut self,
        canvas: &mut WindowCanvas,
        coords: Coordinates,
        frame: u32,
    ) -> Result<Option<InspectorAction>> {
        let (w, h) = coords.points;
        if w == 0 || h == 0 {
            return Ok(None);
        }
        self.input.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, vec2(w as f32, h as f32)));
        self.input.time = Some(self.started.elapsed().as_secs_f64());
        self.input
            .viewports
            .entry(ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(coords.pixels.0 as f32 / w as f32);
        let mut action = None;
        let output = self.context.run(self.input.take(), |ctx| {
            let next = self.pane.show(ctx, Self::area(coords), frame);
            if next.is_some() {
                action = next;
            }
        });
        let video = canvas.window().subsystem();
        if let Some(ime) = &output.platform_output.ime {
            if !self.text_focus {
                video.text_input().start();
            }
            let r = ime.cursor_rect;
            video.text_input().set_rect(sdl2::rect::Rect::new(
                r.min.x as i32,
                r.min.y as i32,
                1,
                r.height().ceil().max(1.) as u32,
            ));
            self.text_focus = true;
        } else if self.text_focus {
            video.text_input().stop();
            self.text_focus = false;
            self.composing = false;
        }
        for cmd in &output.platform_output.commands {
            if let egui::OutputCommand::CopyText(s) = cmd {
                video
                    .clipboard()
                    .set_clipboard_text(s)
                    .map_err(anyhow::Error::msg)?;
            }
        }
        if self.pointer.is_some() {
            let icon = match output.platform_output.cursor_icon {
                egui::CursorIcon::PointingHand => SystemCursor::Hand,
                egui::CursorIcon::Grab | egui::CursorIcon::Grabbing => SystemCursor::SizeAll,
                egui::CursorIcon::ResizeHorizontal => SystemCursor::SizeWE,
                egui::CursorIcon::Crosshair => SystemCursor::Crosshair,
                _ => SystemCursor::Arrow,
            };
            self.set_cursor(icon);
        }
        self.repaint = output
            .viewport_output
            .get(&ViewportId::ROOT)
            .and_then(|v| Instant::now().checked_add(v.repaint_delay));
        let jobs = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);
        self.painter.paint(
            canvas,
            &jobs,
            output.textures_delta,
            output.pixels_per_point,
        )?;
        Ok(action)
    }
}

fn modifiers(m: Mod) -> Modifiers {
    let ctrl = m.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD);
    let mac_cmd = m.intersects(Mod::LGUIMOD | Mod::RGUIMOD);
    Modifiers {
        alt: m.intersects(Mod::LALTMOD | Mod::RALTMOD),
        ctrl,
        shift: m.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD),
        mac_cmd,
        command: if cfg!(target_os = "macos") {
            mac_cmd
        } else {
            ctrl
        },
    }
}
