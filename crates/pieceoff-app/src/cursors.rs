//! Live cursors: where the other players are pointing, and sending our own
//! pointer. Positions are board coordinates (source-image pixels from the
//! board's top-left), so every player sees a cursor over the same spot on
//! the table whatever their pan and zoom.

use super::*;

/// Most pointer updates sent per second.
pub(crate) const CURSOR_SEND_INTERVAL: f64 = 1.0 / 15.0;
/// How long a remote cursor glides to its newest position: about one update
/// interval, so motion looks continuous rather than stepping.
const GLIDE_SECONDS: f64 = 0.1;
/// A cursor with no update for this long is gone (the player stopped moving
/// or their "left the board" message was lost).
pub(crate) const CURSOR_FADE_AFTER: f64 = 3.0;
/// The last part of that time is a fade.
const FADE_SECONDS: f64 = 0.5;

pub(crate) struct RemoteCursor {
    pub(crate) name: String,
    pub(crate) color: Color32,
    from: Pos2,
    to: Pos2,
    updated_at: f64,
}

impl RemoteCursor {
    /// Where the cursor is drawn at `now`, gliding from the previous update.
    pub(crate) fn position(&self, now: f64) -> Pos2 {
        let t = ((now - self.updated_at) / GLIDE_SECONDS).clamp(0.0, 1.0) as f32;
        // Ease out so the glide settles instead of stopping dead.
        let t = 1.0 - (1.0 - t) * (1.0 - t);
        self.from.lerp(self.to, t)
    }

    /// 1 while fresh, falling to 0 at `CURSOR_FADE_AFTER`.
    pub(crate) fn opacity(&self, now: f64) -> f32 {
        let left = CURSOR_FADE_AFTER - (now - self.updated_at);
        (left / FADE_SECONDS).clamp(0.0, 1.0) as f32
    }
}

#[derive(Default)]
pub(crate) struct LiveCursors {
    /// Other players' pointers by client ID.
    pub(crate) remote: HashMap<String, RemoteCursor>,
    /// The position we last sent, or `None` after saying it is hidden.
    sent: Option<Pos2>,
    sent_at: Option<f64>,
}

/// A colour of its own for each player, the same on every client.
pub(crate) fn player_color(client_id: &str) -> Color32 {
    let hue = (pieceoff_core::fnv1a(client_id) % 360) as f32 / 360.0;
    egui::ecolor::HsvaGamma {
        h: hue,
        s: 0.7,
        v: 1.0,
        a: 1.0,
    }
    .into()
}

impl LiveCursors {
    pub(crate) fn update(&mut self, client_id: String, name: String, x: f32, y: f32, now: f64) {
        let target = Pos2::new(x, y);
        match self.remote.get_mut(&client_id) {
            Some(cursor) => {
                cursor.from = cursor.position(now);
                cursor.to = target;
                cursor.updated_at = now;
                cursor.name = name;
            }
            None => {
                let color = player_color(&client_id);
                self.remote.insert(
                    client_id,
                    RemoteCursor {
                        name,
                        color,
                        from: target,
                        to: target,
                        updated_at: now,
                    },
                );
            }
        }
    }

    pub(crate) fn hide(&mut self, client_id: &str) {
        self.remote.remove(client_id);
    }

    /// Forget everything, on entering or leaving a room.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Drop cursors that have faded out.
    pub(crate) fn prune(&mut self, now: f64) {
        self.remote
            .retain(|_, cursor| now - cursor.updated_at < CURSOR_FADE_AFTER);
    }

    /// Decide what to tell the room about our pointer: `Some(Some(p))` to
    /// send a position, `Some(None)` to say it left the table, `None` to send
    /// nothing. Updates are rate limited; when one is held back the returned
    /// delay says when to look again.
    pub(crate) fn outgoing(
        &mut self,
        pointer: Option<Pos2>,
        now: f64,
    ) -> (Option<Option<Pos2>>, Option<f64>) {
        if pointer == self.sent {
            return (None, None);
        }
        // Never shown, nothing to hide.
        if pointer.is_none() && self.sent_at.is_none() {
            return (None, None);
        }
        let since = self.sent_at.map_or(f64::INFINITY, |at| now - at);
        if since < CURSOR_SEND_INTERVAL {
            return (None, Some(CURSOR_SEND_INTERVAL - since));
        }
        self.sent = pointer;
        self.sent_at = Some(now);
        (Some(pointer), None)
    }
}

/// An arrow pointer with its tip at `tip`, a fixed size on screen, and the
/// player's name beside it.
pub(crate) fn paint_cursor(
    painter: &egui::Painter,
    tip: Pos2,
    color: Color32,
    name: &str,
    opacity: f32,
) {
    let fill = color.gamma_multiply(opacity);
    let edge = Stroke::new(1.5_f32, Color32::from_black_alpha((220.0 * opacity) as u8));
    let at = |x: f32, y: f32| tip + Vec2::new(x, y);
    let (left, notch, right) = (at(0.0, 18.0), at(5.0, 13.5), at(12.5, 12.5));
    // Two convex halves make the notched arrow; the outline goes round both.
    for triangle in [[tip, left, notch], [tip, notch, right]] {
        painter.add(egui::Shape::convex_polygon(
            triangle.to_vec(),
            fill,
            Stroke::NONE,
        ));
    }
    painter.add(egui::Shape::closed_line(
        vec![tip, left, notch, right],
        edge,
    ));
    // Dark text on the light player colours reads best.
    let text_color = Color32::from_rgb(16, 18, 22).gamma_multiply(opacity);
    let galley = painter.layout_no_wrap(
        name.to_owned(),
        egui::FontId::proportional(12.0),
        text_color,
    );
    let label = Rect::from_min_size(at(11.0, 16.0), galley.size() + Vec2::new(12.0, 6.0));
    painter.rect_filled(
        label,
        label.height() / 2.0,
        color.gamma_multiply(0.9 * opacity),
    );
    painter.rect_stroke(
        label,
        label.height() / 2.0,
        Stroke::new(1.0_f32, Color32::from_black_alpha((120.0 * opacity) as u8)),
        egui::StrokeKind::Inside,
    );
    painter.galley(label.min + Vec2::new(6.0, 3.0), galley, text_color);
}

impl PieceOffApp {
    /// Tell the room where our pointer is on the table (`None`: off it).
    pub(crate) fn share_pointer(&mut self, on_table: Option<Pos2>, now: f64, ctx: &egui::Context) {
        let (send, retry) = self.cursors.outgoing(on_table, now);
        match send {
            Some(Some(point)) => self.send(ClientMessage::Cursor {
                x: point.x,
                y: point.y,
            }),
            Some(None) => self.send(ClientMessage::CursorHidden),
            None => {}
        }
        if let Some(delay) = retry {
            ctx.request_repaint_after(Duration::from_secs_f64(delay));
        }
    }

    /// Draw the other players' cursors over the pieces.
    pub(crate) fn paint_remote_cursors(
        &mut self,
        painter: &egui::Painter,
        to_screen: impl Fn(Pos2) -> Pos2,
        now: f64,
        ctx: &egui::Context,
    ) {
        self.cursors.prune(now);
        if self.cursors.remote.is_empty() {
            return;
        }
        // Keep gliding and fading; a cursor left alone disappears on time.
        ctx.request_repaint_after(Duration::from_millis(33));
        if self.settings.hide_peer_cursors {
            return;
        }
        let mut cursors: Vec<_> = self.cursors.remote.iter().collect();
        // A steady order, so overlapping cursors do not flicker.
        cursors.sort_unstable_by(|a, b| a.0.cmp(b.0));
        for (_, cursor) in cursors {
            paint_cursor(
                painter,
                to_screen(cursor.position(now)),
                cursor.color,
                &cursor.name,
                cursor.opacity(now),
            );
        }
    }
}
