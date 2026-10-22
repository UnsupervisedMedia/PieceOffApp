//! Room chat and board pings. Both come from the server, which relays them
//! to everyone in the room (the sender included) and stores neither in the
//! room snapshot; a player who joins gets the recent chat once.

use super::*;
use eframe::egui::{RichText, text::LayoutJob};
use pieceoff_core::{ChatLine, MAX_CHAT_CHARS};
use std::collections::VecDeque;

/// How long a ping stays on the table.
pub(crate) const PING_SECONDS: f64 = 3.0;
/// The last part of that time is a fade.
const PING_FADE_SECONDS: f64 = 0.6;
/// Lines kept on this client, including our own notes.
const MAX_CHAT_ENTRIES: usize = 200;
const CHAT_WIDTH: f32 = 320.0;

pub(crate) enum ChatEntry {
    /// Something a player said, as the server stamped it.
    Line(ChatLine),
    /// A note this client adds from room events: joins, leaves, restarts.
    Note { text: String, at_ms: u64 },
}

impl ChatEntry {
    fn at_ms(&self) -> u64 {
        match self {
            Self::Line(line) => line.sent_at_ms,
            Self::Note { at_ms, .. } => *at_ms,
        }
    }
}

pub(crate) struct ActivePing {
    pub(crate) name: String,
    pub(crate) color: Color32,
    /// Board coordinates.
    pub(crate) at: Pos2,
    pub(crate) started: f64,
}

#[derive(Default)]
pub(crate) struct RoomChat {
    /// Expanded to show the history and the text box; collapsed it is a
    /// small button with an unread count.
    pub(crate) open: bool,
    pub(crate) entries: VecDeque<ChatEntry>,
    pub(crate) draft: String,
    /// Lines from other players that arrived while collapsed.
    pub(crate) unread: usize,
    /// Put the cursor in the text box on the next frame.
    pub(crate) focus_requested: bool,
    pub(crate) pings: Vec<ActivePing>,
    /// The ping key was pressed; it drops where the pointer is this frame.
    pub(crate) ping_requested: bool,
    /// The text box had the keyboard last frame. egui drops focus as soon
    /// as Escape arrives, before the shortcuts run, so without this the
    /// Escape that leaves the chat would also clear the selection.
    pub(crate) typing: bool,
}

pub(crate) fn wall_clock_ms() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl RoomChat {
    /// Forget everything, on leaving a room or entering another.
    pub(crate) fn reset(&mut self) {
        let open = self.open;
        *self = Self::default();
        self.open = open;
    }

    pub(crate) fn show(&mut self) {
        self.open = true;
        self.unread = 0;
    }

    pub(crate) fn hide(&mut self) {
        self.open = false;
    }

    fn push(&mut self, entry: ChatEntry) {
        if self.entries.len() >= MAX_CHAT_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    /// A line from the server. Ours never counts as unread.
    pub(crate) fn receive(&mut self, line: ChatLine, own_client_id: Option<&str>) {
        if !self.open && own_client_id != Some(line.client_id.as_str()) {
            self.unread += 1;
        }
        self.push(ChatEntry::Line(line));
    }

    /// The room's recent chat, sent on joining. On a reconnect most of it is
    /// already here, so only lines we have not seen are added; those from
    /// other players count as unread like any other. Our own earlier lines
    /// came from an older connection, so they are recognised by name.
    pub(crate) fn history(&mut self, lines: Vec<ChatLine>, own_name: &str) {
        let fresh: Vec<ChatLine> = lines
            .into_iter()
            .filter(|line| {
                !self
                    .entries
                    .iter()
                    .any(|entry| matches!(entry, ChatEntry::Line(seen) if seen == line))
            })
            .collect();
        if fresh.is_empty() {
            return;
        }
        if !self.open {
            self.unread += fresh.iter().filter(|line| line.name != own_name).count();
        }
        // History predates anything said since we joined; keep time order.
        let mut entries: Vec<ChatEntry> = self.entries.drain(..).collect();
        entries.extend(fresh.into_iter().map(ChatEntry::Line));
        entries.sort_by_key(ChatEntry::at_ms);
        for entry in entries {
            self.push(entry);
        }
    }

    pub(crate) fn note(&mut self, text: impl Into<String>) {
        self.push(ChatEntry::Note {
            text: text.into(),
            at_ms: wall_clock_ms(),
        });
    }

    pub(crate) fn add_ping(&mut self, name: String, x: f32, y: f32, now: f64) {
        self.pings.push(ActivePing {
            color: player_color(&name),
            name,
            at: Pos2::new(x, y),
            started: now,
        });
    }

    pub(crate) fn prune_pings(&mut self, now: f64) {
        self.pings.retain(|ping| now - ping.started < PING_SECONDS);
    }
}

/// An expanding ring marker in the pinger's colour with their name, at a
/// fixed size on screen. `inside` is false when the spot is off screen: then
/// the marker sits at the nearest edge without rings, so it still says
/// "over here".
pub(crate) fn paint_ping(
    painter: &egui::Painter,
    center: Pos2,
    ping: &ActivePing,
    now: f64,
    inside: bool,
) {
    let age = (now - ping.started).max(0.0);
    let opacity = ((PING_SECONDS - age) / PING_FADE_SECONDS).clamp(0.0, 1.0) as f32;
    let shadow = Color32::from_black_alpha((200.0 * opacity) as u8);
    if inside {
        // Two rings a half-period apart, each growing and fading.
        for offset in [0.0, 0.5] {
            let phase = ((age * 1.25 + offset) % 1.0) as f32;
            let radius = 8.0 + phase * 38.0;
            let alpha = (1.0 - phase) * opacity;
            painter.circle_stroke(
                center,
                radius,
                Stroke::new(4.0_f32, Color32::from_black_alpha((90.0 * alpha) as u8)),
            );
            painter.circle_stroke(
                center,
                radius,
                Stroke::new(2.5_f32, ping.color.gamma_multiply(alpha)),
            );
        }
    }
    painter.circle_filled(center, 6.0, ping.color.gamma_multiply(opacity));
    painter.circle_stroke(center, 6.0, Stroke::new(1.5_f32, shadow));
    let text_color = Color32::from_rgb(16, 18, 22).gamma_multiply(opacity);
    let galley = painter.layout_no_wrap(
        ping.name.clone(),
        egui::FontId::proportional(12.0),
        text_color,
    );
    let size = galley.size() + Vec2::new(12.0, 6.0);
    let label = Rect::from_min_size(center + Vec2::new(-size.x / 2.0, -12.0 - size.y), size);
    painter.rect_filled(
        label,
        label.height() / 2.0,
        ping.color.gamma_multiply(0.9 * opacity),
    );
    painter.rect_stroke(
        label,
        label.height() / 2.0,
        Stroke::new(1.0_f32, shadow),
        egui::StrokeKind::Inside,
    );
    painter.galley(label.min + Vec2::new(6.0, 3.0), galley, text_color);
}

impl PieceOffApp {
    /// Send the draft, if there is anything in it.
    pub(crate) fn send_chat(&mut self) {
        let text = self.chat.draft.trim().to_owned();
        if text.is_empty() {
            return;
        }
        if !self.connected {
            self.status = "Not connected to the room; the message was not sent.".into();
            return;
        }
        self.send(ClientMessage::Chat { text });
        self.chat.draft.clear();
    }

    /// Drop a ping at `on_table` (board coordinates) if the ping key was
    /// pressed this frame.
    pub(crate) fn maybe_ping(&mut self, on_table: Option<Pos2>) {
        if !std::mem::take(&mut self.chat.ping_requested) {
            return;
        }
        match on_table {
            Some(point) => self.send(ClientMessage::Ping {
                x: point.x,
                y: point.y,
            }),
            None => {
                self.status = format!(
                    "Point at the table, then press {} to ping.",
                    self.controls.describe(Action::Ping)
                )
            }
        }
    }

    /// Draw the live pings over the pieces.
    pub(crate) fn paint_pings(
        &mut self,
        painter: &egui::Painter,
        canvas: Rect,
        to_screen: impl Fn(Pos2) -> Pos2,
        now: f64,
        ctx: &egui::Context,
    ) {
        self.chat.prune_pings(now);
        if self.chat.pings.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(16));
        let edge = canvas.shrink(24.0);
        for ping in &self.chat.pings {
            let point = to_screen(ping.at);
            let inside = canvas.contains(point);
            let shown = if inside { point } else { edge.clamp(point) };
            paint_ping(painter, shown, ping, now, inside);
        }
    }

    /// The chat box in the bottom-left corner of the board: a button with an
    /// unread count when collapsed, the history and a text box when open.
    pub(crate) fn chat_overlay(&mut self, ctx: &egui::Context) {
        let now_ms = wall_clock_ms();
        let hint = self.hint_with_keys("Room chat", Action::FocusChat);
        let send_hint = format!(
            "Say something · Enter sends · Esc goes back to the board{}",
            if self.controls.bindings_for(Action::Ping).is_empty() {
                String::new()
            } else {
                format!(
                    " · {} pings the table",
                    self.controls.describe(Action::Ping)
                )
            }
        );
        egui::Area::new(egui::Id::new("room-chat"))
            .anchor(egui::Align2::LEFT_BOTTOM, Vec2::new(12.0, -14.0))
            .constrain_to(ctx.available_rect())
            .show(ctx, |ui| {
                if !self.chat.open {
                    let unread = self.chat.unread;
                    let text = if unread > 0 {
                        RichText::new(format!("Chat · {unread} new"))
                            .color(ACCENT)
                            .strong()
                    } else {
                        RichText::new("Chat").color(Color32::WHITE)
                    };
                    let button = egui::Button::new(text)
                        .wrap_mode(egui::TextWrapMode::Extend)
                        .fill(Color32::from_black_alpha(215))
                        .corner_radius(14);
                    if ui.add(button).on_hover_text(hint).clicked() {
                        self.chat.show();
                        self.chat.focus_requested = true;
                    }
                    return;
                }
                egui::Frame::new()
                    .fill(Color32::from_black_alpha(205))
                    .corner_radius(10)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(CHAT_WIDTH);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Chat").strong().color(Color32::WHITE));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .small_button("Hide")
                                        .on_hover_text("Collapse the chat")
                                        .clicked()
                                    {
                                        self.chat.hide();
                                    }
                                },
                            );
                        });
                        egui::ScrollArea::vertical()
                            .max_height(170.0)
                            .auto_shrink([false, true])
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                if self.chat.entries.is_empty() {
                                    ui.weak("Nothing said yet.");
                                }
                                for entry in &self.chat.entries {
                                    ui.add(egui::Label::new(chat_layout(entry, now_ms)).wrap());
                                }
                            });
                        let edit = egui::TextEdit::singleline(&mut self.chat.draft)
                            .hint_text("Say something…")
                            .char_limit(MAX_CHAT_CHARS)
                            .desired_width(f32::INFINITY);
                        let response = ui.add(edit).on_hover_text(send_hint);
                        self.chat.typing = response.has_focus();
                        if std::mem::take(&mut self.chat.focus_requested) {
                            response.request_focus();
                        }
                        // Enter in a one-line box ends editing; that is the send.
                        if response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter))
                        {
                            self.send_chat();
                        }
                    });
                // Keep "2m ago" honest while the history is showing.
                ctx.request_repaint_after(Duration::from_secs(5));
            });
    }
}

/// One history line: when, who (in their colour), and what.
fn chat_layout(entry: &ChatEntry, now_ms: u64) -> LayoutJob {
    let mut job = LayoutJob::default();
    let format = |color: Color32, size: f32| egui::TextFormat {
        font_id: egui::FontId::proportional(size),
        color,
        ..Default::default()
    };
    let age = Duration::from_millis(now_ms.saturating_sub(entry.at_ms()));
    job.append(&format_ago(age), 0.0, format(Color32::from_gray(130), 11.0));
    match entry {
        ChatEntry::Line(line) => {
            job.append(&line.name, 6.0, format(player_color(&line.name), 13.5));
            job.append(": ", 0.0, format(Color32::from_gray(170), 13.5));
            job.append(&line.text, 0.0, format(Color32::from_gray(235), 13.5));
        }
        ChatEntry::Note { text, .. } => {
            let mut italic = format(Color32::from_gray(160), 13.0);
            italic.italics = true;
            job.append(text, 6.0, italic);
        }
    }
    job
}
