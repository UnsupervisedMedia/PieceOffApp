//! Replaying the room's solve timelapse locally. The server keeps a log of
//! where pieces came to rest (see `pieceoff_core::Timelapse`); this steps
//! through it on a copy of the room, never touching the live one.

use super::*;
use eframe::egui::RichText;
use pieceoff_core::Timelapse;

/// Whole-replay lengths offered, in seconds.
pub(crate) const REPLAY_LENGTHS: [u32; 4] = [20, 30, 45, 60];
const DEFAULT_REPLAY_SECONDS: u32 = 30;
/// How close (in source pixels) a piece must be to its cell, or to a
/// neighbour's matching edge, to count as placed or joined. Logged positions
/// are rounded to whole pixels.
const JOIN_TOLERANCE: f32 = 1.5;

/// A replay in progress. Time is measured in steps: step `n` means the first
/// `n` frames of the log have landed, and fractions animate the next one.
pub(crate) struct Replay {
    log: Timelapse,
    /// Where the replay is, in steps from the start (0..=frames).
    pub(crate) position: f64,
    pub(crate) playing: bool,
    /// How long the whole replay takes at the chosen speed.
    pub(crate) seconds: u32,
    last_tick: Option<f64>,
    /// Positions after `applied` frames, by piece id.
    settled: HashMap<u32, (f32, f32)>,
    applied: usize,
}

impl Replay {
    /// A replay of `log` for a room with `pieces`, or why there is none.
    pub(crate) fn new(log: Timelapse, pieces: &[Piece]) -> Result<Self, &'static str> {
        if log.frames.is_empty() {
            return Err("Nothing to replay yet: no piece has been put down in this solve.");
        }
        let fits = log.start.len() == pieces.len()
            && log.start.iter().all(|[id, ..]| {
                u32::try_from(*id).is_ok_and(|id| pieces.iter().any(|piece| piece.id == id))
            });
        if !fits {
            return Err("The timelapse does not match this puzzle.");
        }
        let mut replay = Self {
            log,
            position: 0.0,
            playing: true,
            seconds: DEFAULT_REPLAY_SECONDS,
            last_tick: None,
            settled: HashMap::new(),
            applied: 0,
        };
        replay.reset();
        Ok(replay)
    }

    fn reset(&mut self) {
        self.settled = self
            .log
            .start
            .iter()
            .map(|[id, x, y]| (*id as u32, (*x as f32, *y as f32)))
            .collect();
        self.applied = 0;
    }

    pub(crate) fn steps(&self) -> usize {
        self.log.frames.len()
    }

    pub(crate) fn condensed(&self) -> bool {
        self.log.condensed
    }

    /// The solve-clock time of the latest step shown.
    pub(crate) fn solve_time(&self) -> Duration {
        let shown = (self.position.floor() as usize).min(self.steps());
        let at = shown
            .checked_sub(1)
            .and_then(|index| self.log.frames.get(index))
            .map_or(0, |frame| frame.at_ms);
        Duration::from_millis(at)
    }

    pub(crate) fn finished(&self) -> bool {
        self.position >= self.steps() as f64
    }

    /// Jump to `position` steps, clamped to the log.
    pub(crate) fn seek(&mut self, position: f64) {
        self.position = position.clamp(0.0, self.steps() as f64);
        self.last_tick = None;
    }

    pub(crate) fn toggle(&mut self) {
        if self.finished() {
            self.seek(0.0);
        }
        self.playing = !self.playing;
        self.last_tick = None;
    }

    /// Move the replay on to wall-clock time `now` (seconds).
    pub(crate) fn advance(&mut self, now: f64) {
        if !self.playing {
            self.last_tick = None;
            return;
        }
        if let Some(last) = self.last_tick {
            let per_second = self.steps() as f64 / f64::from(self.seconds.max(1));
            self.position =
                (self.position + (now - last).max(0.0) * per_second).min(self.steps() as f64);
        }
        self.last_tick = Some(now);
        if self.finished() {
            self.playing = false;
            self.last_tick = None;
        }
    }

    /// Every piece's position at the current step, the next step's pieces
    /// part-way along.
    pub(crate) fn positions(&mut self) -> HashMap<u32, (f32, f32)> {
        let whole = (self.position.floor() as usize).min(self.steps());
        if whole < self.applied {
            self.reset();
        }
        while self.applied < whole {
            for [id, x, y] in &self.log.frames[self.applied].moves {
                self.settled.insert(*id as u32, (*x as f32, *y as f32));
            }
            self.applied += 1;
        }
        let mut positions = self.settled.clone();
        let fraction = (self.position - whole as f64) as f32;
        if let (Some(next), true) = (self.log.frames.get(whole), fraction > 0.0) {
            for [id, x, y] in &next.moves {
                let id = *id as u32;
                let from = positions
                    .get(&id)
                    .copied()
                    .unwrap_or((*x as f32, *y as f32));
                let to = (*x as f32, *y as f32);
                positions.insert(
                    id,
                    (
                        from.0 + (to.0 - from.0) * fraction,
                        from.1 + (to.1 - from.1) * fraction,
                    ),
                );
            }
        }
        positions
    }

    /// The live room as it looked at this point of the replay: positions
    /// from the log, with placement and joins worked out from them.
    pub(crate) fn view_of(&mut self, live: &RoomSnapshot) -> RoomSnapshot {
        let positions = self.positions();
        let mut view = live.clone();
        view.holders.clear();
        for piece in &mut view.pieces {
            if let Some((x, y)) = positions.get(&piece.id) {
                piece.x = *x;
                piece.y = *y;
            }
            piece.held_by = None;
        }
        settle_view(&mut view);
        view
    }
}

/// Work out which pieces sit in their cells and which neighbours meet edge
/// to edge, from positions alone, so the replay draws and counts progress
/// the way the room did.
pub(crate) fn settle_view(view: &mut RoomSnapshot) {
    let (cw, ch) = (view.cell_width(), view.cell_height());
    let near = |a: f32, b: f32| (a - b).abs() <= JOIN_TOLERANCE;
    let index: HashMap<(u16, u16), usize> = view
        .pieces
        .iter()
        .enumerate()
        .map(|(index, piece)| ((piece.row, piece.column), index))
        .collect();
    let mut parent: Vec<usize> = (0..view.pieces.len()).collect();
    fn root(parent: &mut [usize], mut node: usize) -> usize {
        while parent[node] != node {
            parent[node] = parent[parent[node]];
            node = parent[node];
        }
        node
    }
    for (index_a, piece) in view.pieces.iter().enumerate() {
        for (row, column, dx, dy) in [
            (piece.row, piece.column + 1, cw, 0.0),
            (piece.row + 1, piece.column, 0.0, ch),
        ] {
            let Some(&index_b) = index.get(&(row, column)) else {
                continue;
            };
            let other = &view.pieces[index_b];
            if near(other.x - piece.x, dx) && near(other.y - piece.y, dy) {
                let (a, b) = (root(&mut parent, index_a), root(&mut parent, index_b));
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let ids: Vec<u32> = view.pieces.iter().map(|piece| piece.id).collect();
    for index in 0..view.pieces.len() {
        let group = ids[root(&mut parent, index)];
        let piece = &mut view.pieces[index];
        piece.group_id = group;
        piece.placed =
            near(piece.x, f32::from(piece.column) * cw) && near(piece.y, f32::from(piece.row) * ch);
    }
}

impl PieceOffApp {
    /// Ask the server for the timelapse; the replay opens when it arrives.
    pub(crate) fn request_timelapse(&mut self) {
        if self.network.is_none() {
            self.status = "Connect to the room to fetch its timelapse.".into();
            return;
        }
        self.timelapse_requested = true;
        self.status = "Fetching the timelapse…".into();
        self.send(ClientMessage::RequestTimelapse);
    }

    /// The server's answer to `request_timelapse`.
    pub(crate) fn receive_timelapse(&mut self, log: Timelapse) {
        if !std::mem::take(&mut self.timelapse_requested) {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        match Replay::new(log, &snapshot.pieces) {
            Ok(replay) => {
                self.status = format!("Replaying {} steps of this solve.", replay.steps());
                self.selected_pieces.clear();
                self.clear_active_gesture();
                self.replay = Some(replay);
            }
            Err(reason) => self.status = reason.into(),
        }
    }

    /// Play, pause, scrub, speed and close, docked at the bottom of the board.
    pub(crate) fn replay_controls(&mut self, ctx: &egui::Context) {
        let Some(replay) = self.replay.as_mut() else {
            return;
        };
        let mut close = false;
        egui::Window::new("Timelapse")
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -16.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Timelapse").strong());
                    let icon = if replay.playing { "⏸" } else { "▶" };
                    let hint = if replay.playing { "Pause" } else { "Play" };
                    if ui
                        .button(RichText::new(icon).size(16.0))
                        .on_hover_text(hint)
                        .clicked()
                    {
                        replay.toggle();
                    }
                    let steps = replay.steps() as f64;
                    let mut position = replay.position;
                    ui.spacing_mut().slider_width = 280.0;
                    let slider = ui.add(
                        egui::Slider::new(&mut position, 0.0..=steps)
                            .show_value(false)
                            .clamping(egui::SliderClamping::Always),
                    );
                    if slider.changed() {
                        replay.seek(position);
                    }
                    ui.label(
                        RichText::new(format!(
                            "{} / {} · {}",
                            replay.position.floor() as usize,
                            replay.steps(),
                            format_elapsed(replay.solve_time())
                        ))
                        .monospace(),
                    )
                    .on_hover_text("Steps shown, and the solve clock at that step");
                    egui::ComboBox::from_id_salt("replay-speed")
                        .width(80.0)
                        .selected_text(format!("{} s", replay.seconds))
                        .show_ui(ui, |ui| {
                            for seconds in REPLAY_LENGTHS {
                                ui.selectable_value(
                                    &mut replay.seconds,
                                    seconds,
                                    format!("Whole solve in {seconds} s"),
                                );
                            }
                        })
                        .response
                        .on_hover_text("How long the whole replay takes");
                    if ui
                        .button("Close")
                        .on_hover_text("Back to the live room")
                        .clicked()
                    {
                        close = true;
                    }
                });
                if replay.condensed() {
                    ui.weak("A long solve: some steps were merged to keep the log small.");
                }
            });
        let now = ctx.input(|input| input.time);
        if let Some(replay) = self.replay.as_mut() {
            replay.advance(now);
            if replay.playing {
                ctx.request_repaint();
            }
        }
        if close {
            self.replay = None;
            self.status = "Back to the live room.".into();
        }
    }
}
