use super::*;
use std::sync::atomic::{AtomicI64, Ordering};

/// An outline thinned to every `stride`th point, with its triangulation.
pub(crate) struct ShapeDetail {
    pub(crate) outline: Vec<[f32; 2]>,
    pub(crate) triangles: Vec<[usize; 3]>,
}

/// Screen points per outline sample below which a piece is drawn from a
/// thinned outline: shorter segments than this are invisible.
const MIN_SEGMENT_POINTS: f32 = 2.2;
/// Thinning factors, finest first. Each divides the samples per edge, so
/// piece corners always survive.
const DETAIL_STRIDES: [usize; 5] = [1, 2, 4, 8, 16];

pub(crate) struct CachedPieceShape {
    /// Full-detail outline, used for picking and large pieces.
    pub(crate) outline: Vec<[f32; 2]>,
    /// Outline bounds in piece-local source pixels.
    pub(crate) bounds: Rect,
    /// Triangulated levels of detail, built the first time each is drawn:
    /// ear clipping the full outline of every piece up front would delay
    /// entering a large room.
    levels: [std::cell::OnceCell<Option<ShapeDetail>>; DETAIL_STRIDES.len()],
}

impl CachedPieceShape {
    pub(crate) fn new(outline: Vec<[f32; 2]>) -> Self {
        let mut bounds = Rect::NOTHING;
        for point in &outline {
            bounds.extend_with(Pos2::new(point[0], point[1]));
        }
        Self {
            outline,
            bounds,
            levels: Default::default(),
        }
    }

    fn level(&self, index: usize) -> Option<&ShapeDetail> {
        self.levels[index]
            .get_or_init(|| {
                let stride = DETAIL_STRIDES[index];
                let outline: Vec<[f32; 2]> = self.outline.iter().step_by(stride).copied().collect();
                let triangles = triangulate_polygon(&outline)?;
                Some(ShapeDetail { outline, triangles })
            })
            .as_ref()
    }

    /// The outline and triangulation to draw when the piece's perimeter is
    /// about `perimeter_points` screen points long: the coarsest level that
    /// still has enough samples, or a finer one if that cannot be
    /// triangulated.
    pub(crate) fn detail(&self, perimeter_points: f32) -> Option<&ShapeDetail> {
        let wanted = (perimeter_points / MIN_SEGMENT_POINTS).max(4.0);
        let coarsest = DETAIL_STRIDES
            .iter()
            .rposition(|stride| (self.outline.len() / stride) as f32 >= wanted)
            .unwrap_or(0);
        (0..=coarsest).rev().find_map(|index| self.level(index))
    }
}

pub(crate) struct ActiveDrag {
    pub(crate) anchor_piece_id: u32,
    pub(crate) pointer_offset: Vec2,
    pub(crate) origins: Vec<(u32, f32, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ViewCommand {
    Fit,
    Board,
    LoosePieces,
    /// Zoom about the middle of the board by this factor.
    ZoomBy(f32),
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum PieceFilter {
    #[default]
    All,
    Edges,
    Interior,
}

impl PieceFilter {
    pub(crate) fn includes(self, piece: &Piece, rows: u16, columns: u16) -> bool {
        let is_edge = piece.row == 0
            || piece.column == 0
            || piece.row == rows.saturating_sub(1)
            || piece.column == columns.saturating_sub(1);
        match self {
            Self::All => true,
            Self::Edges => is_edge,
            Self::Interior => !is_edge,
        }
    }
}

pub(crate) fn piece_visible_for_filter(
    filter: PieceFilter,
    piece: &Piece,
    width: u32,
    rows: u16,
    columns: u16,
) -> bool {
    piece.placed || piece.x < width as f32 || filter.includes(piece, rows, columns)
}

/// Something the board asks the app to do once the room snapshot is back in
/// place.
enum BoardAction {
    PlayAgain,
    WatchTimelapse,
}

/// How the board responds to the pointer.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoardMode {
    /// A player: pieces and holders move.
    Play,
    /// A spectator: the live room, looked at but not touched.
    Watch,
    /// A timelapse replay over a copy of the room.
    Replay,
}

/// Maps source-image pixels to screen points.
#[derive(Clone, Copy)]
struct BoardView {
    origin: Pos2,
    scale: f32,
}

impl BoardView {
    fn to_screen(self, x: f32, y: f32) -> Pos2 {
        Pos2::new(
            self.origin.x + x * self.scale,
            self.origin.y + y * self.scale,
        )
    }

    fn to_board(self, point: Pos2) -> Pos2 {
        ((point - self.origin) / self.scale).to_pos2()
    }
}

/// Back-to-front paint layer: placed pieces lie flat, loose pieces sit above
/// them, and selected or held pieces float on top.
fn piece_layer(piece: &Piece, selected: &std::collections::HashSet<u32>) -> u8 {
    if piece.placed {
        0
    } else if piece.held_by.is_some() {
        3
    } else if selected.contains(&piece.id) {
        2
    } else {
        1
    }
}

pub(crate) fn paint_order(
    snapshot: &RoomSnapshot,
    selected: &std::collections::HashSet<u32>,
) -> Vec<usize> {
    let mut order: Vec<usize> = (0..snapshot.pieces.len()).collect();
    order.sort_by_key(|&index| piece_layer(&snapshot.pieces[index], selected));
    order
}

/// The topmost loose piece under `at` (source-image pixels).
fn piece_at(
    snapshot: &RoomSnapshot,
    order: &[usize],
    shapes: &std::collections::HashMap<u32, CachedPieceShape>,
    filter: PieceFilter,
    at: Pos2,
    margin: f32,
) -> Option<u32> {
    let candidates = || {
        order
            .iter()
            .rev()
            .map(|&index| &snapshot.pieces[index])
            .filter(|piece| {
                !piece.placed
                    && piece_visible_for_filter(
                        filter,
                        piece,
                        snapshot.width,
                        snapshot.rows,
                        snapshot.columns,
                    )
            })
    };
    let exact = candidates().find(|piece| {
        let Some(shape) = shapes.get(&piece.id) else {
            return false;
        };
        let local = Pos2::new(at.x - piece.x, at.y - piece.y);
        shape.bounds.contains(local) && point_in_polygon(local, &shape.outline)
    });
    if let Some(piece) = exact {
        return Some(piece.id);
    }
    if margin <= 0.0 {
        return None;
    }
    // Nothing directly under the pointer: take the piece whose edge is
    // nearest, within the margin. `min_by` keeps the first (topmost) on ties.
    candidates()
        .filter_map(|piece| {
            let shape = shapes.get(&piece.id)?;
            let local = Pos2::new(at.x - piece.x, at.y - piece.y);
            if !shape.bounds.expand(margin).contains(local) {
                return None;
            }
            let distance = distance_to_outline(local, &shape.outline);
            (distance <= margin).then_some((piece.id, distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

/// Shortest distance from `point` to the closed polygon's edges.
pub(crate) fn distance_to_outline(point: Pos2, polygon: &[[f32; 2]]) -> f32 {
    let mut best = f32::INFINITY;
    let mut previous = polygon.len().saturating_sub(1);
    for current in 0..polygon.len() {
        let a = Pos2::new(polygon[previous][0], polygon[previous][1]);
        let b = Pos2::new(polygon[current][0], polygon[current][1]);
        let edge = b - a;
        let length_sq = edge.length_sq();
        let t = if length_sq > 0.0 {
            ((point - a).dot(edge) / length_sq).clamp(0.0, 1.0)
        } else {
            0.0
        };
        best = best.min(point.distance(a + edge * t));
        previous = current;
    }
    best
}

/// Loose pieces whose outlines touch `area` (board coordinates). Placed
/// pieces and ones another player is holding cannot be picked up, and the
/// current piece filter applies, as for a click.
fn pieces_touching(
    snapshot: &RoomSnapshot,
    shapes: &std::collections::HashMap<u32, CachedPieceShape>,
    filter: PieceFilter,
    area: Rect,
    me: Option<&str>,
) -> Vec<u32> {
    snapshot
        .pieces
        .iter()
        .filter(|piece| {
            !piece.placed
                && piece
                    .held_by
                    .as_deref()
                    .is_none_or(|holder| Some(holder) == me)
                && piece_visible_for_filter(
                    filter,
                    piece,
                    snapshot.width,
                    snapshot.rows,
                    snapshot.columns,
                )
        })
        .filter(|piece| {
            let bounds = shapes.get(&piece.id).map_or_else(
                || {
                    Rect::from_min_size(
                        Pos2::ZERO,
                        Vec2::new(snapshot.cell_width(), snapshot.cell_height()),
                    )
                },
                |shape| shape.bounds,
            );
            bounds
                .translate(Vec2::new(piece.x, piece.y))
                .intersects(area)
        })
        .map(|piece| piece.id)
        .collect()
}

pub(crate) fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

const SNAP_FLASH_SECONDS: f64 = 0.6;

/// Seconds between position updates sent while dragging (about 30 a second).
const MOVE_SEND_INTERVAL: f64 = 1.0 / 30.0;

/// Pieces drawn narrower than this get no outline stroke.
const TINY_PIECE_POINTS: f32 = 7.0;

impl PieceOffApp {
    pub(crate) fn render_board(&mut self, ui: &mut egui::Ui) {
        // The snapshot is lent to the renderer so pieces can be read while
        // gestures update the rest of the app, without cloning it per frame.
        let Some(mut snapshot) = self.snapshot.take() else {
            self.render_media_preview(ui);
            return;
        };
        let action = if let Some(mut replay) = self.replay.take() {
            let mut view = replay.view_of(&snapshot);
            self.replay = Some(replay);
            self.render_room(ui, &mut view, BoardMode::Replay)
        } else {
            let mode = if self.spectating {
                BoardMode::Watch
            } else {
                BoardMode::Play
            };
            self.render_room(ui, &mut snapshot, mode)
        };
        self.snapshot = Some(snapshot);
        match action {
            Some(BoardAction::PlayAgain) => self.restart_puzzle(),
            Some(BoardAction::WatchTimelapse) => self.request_timelapse(),
            None => {}
        }
    }

    /// Rebuild per-room caches on entering a room and track solve progress.
    fn sync_room_state(&mut self, snapshot: &RoomSnapshot, now: f64) {
        if self.shape_cache_room != snapshot.id {
            self.shape_cache.clear();
            for piece in &snapshot.pieces {
                self.shape_cache.insert(
                    piece.id,
                    CachedPieceShape::new(piece_outline(snapshot, piece)),
                );
            }
            self.shape_cache_room.clone_from(&snapshot.id);
            self.placed_seen = snapshot
                .pieces
                .iter()
                .filter(|piece| piece.placed)
                .map(|piece| piece.id)
                .collect();
            self.snap_flashes.clear();
            // A run already finished when we arrived is not ours to record.
            self.solve_recorded = snapshot.stats.finished;
        }
        for piece in &snapshot.pieces {
            if !piece.placed {
                self.placed_seen.remove(&piece.id);
            } else if self.placed_seen.insert(piece.id) {
                self.snap_flashes.push((piece.id, now));
            }
        }
        self.snap_flashes
            .retain(|(_, started)| now - started < SNAP_FLASH_SECONDS);
        if !snapshot.stats.finished {
            // A restart begins a fresh run.
            self.solve_recorded = false;
        } else if !self.solve_recorded {
            self.solve_recorded = true;
            self.record_library_solve(snapshot.stats.elapsed_ms);
        }
    }

    /// A library puzzle finished while we watched: keep its best time.
    fn record_library_solve(&mut self, solve_ms: u64) {
        let Some(id) = self.library_entry_id.clone() else {
            return;
        };
        if let Err(error) = self.library.record_solve(&id, solve_ms) {
            self.status = format!("Could not record the solve time: {error}");
        }
    }

    /// The room's solve time as of now, from the shared clock; `None` before
    /// anyone has picked up a piece.
    pub(crate) fn solve_time(snapshot: &RoomSnapshot) -> Option<Duration> {
        let stats = &snapshot.stats;
        stats
            .started
            .then(|| Duration::from_millis(stats.elapsed_at(server_clock_ms())))
    }

    fn render_room(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &mut RoomSnapshot,
        mode: BoardMode,
    ) -> Option<BoardAction> {
        let interactive = mode == BoardMode::Play;
        let Some(texture_id) = self
            .still_assist_texture
            .as_ref()
            .or(self.texture.as_ref())
            .map(|texture| texture.id())
        else {
            ui.centered_and_justified(|ui| {
                ui.label("Waiting for the puzzle image or first video frame…");
            });
            return None;
        };
        let now = ui.input(|input| input.time);
        // A replay is a copy: its pieces must not flash or record a finish.
        if mode != BoardMode::Replay {
            self.sync_room_state(snapshot, now);
        }
        let available = ui.available_size().max(Vec2::splat(1.0));
        let (response, painter) = ui.allocate_painter(available, Sense::click_and_drag());
        let canvas = response.rect;
        if self.show_reference && !self.is_video {
            let image_rect = fit_media_rect(
                canvas,
                Vec2::new(snapshot.width as f32, snapshot.height as f32),
            );
            painter.image(
                texture_id,
                image_rect,
                cover_uv_rect(
                    snapshot.width as f32,
                    snapshot.height as f32,
                    self.media_size.0 as f32,
                    self.media_size.1 as f32,
                ),
                Color32::WHITE,
            );
            paint_pill(
                &painter,
                canvas.center_top() + Vec2::new(0.0, 12.0),
                "Reference image · hide it to keep puzzling",
                Color32::WHITE,
            );
            self.share_pointer(None, now, ui.ctx());
            self.chat.ping_requested = false;
            return None;
        }
        let tray_layout = piece_tray_layout(
            snapshot.width,
            snapshot.height,
            snapshot.rows,
            snapshot.columns,
        );
        let content_size = Vec2::new(tray_layout.content_width, tray_layout.content_height);
        let base_scale = (canvas.width() * 0.92 / content_size.x)
            .min(canvas.height() * 0.92 / content_size.y)
            .max(0.05);
        let image_size = Vec2::new(snapshot.width as f32, snapshot.height as f32);
        // The board never changes shape; whatever media is current is
        // centre-cropped to cover it.
        let media_uv = cover_uv_rect(
            image_size.x,
            image_size.y,
            self.media_size.0 as f32,
            self.media_size.1 as f32,
        );

        // View changes apply before layout so they take effect this frame.
        let locked = self.settings.lock_view;
        if response.hovered() && !locked {
            let (scroll_y, pointer) = ui.input(|input| {
                (
                    input.smooth_scroll_delta.y,
                    input.pointer.hover_pos().unwrap_or(canvas.center()),
                )
            });
            if scroll_y.abs() > 0.0 {
                (self.board_zoom, self.board_pan) = zoom_at_pointer(
                    self.board_zoom,
                    self.board_pan,
                    canvas.center(),
                    pointer,
                    self.settings.mouse.adjust_scroll(scroll_y),
                );
            }
        }
        if let Some(command) = self.view_command.take().filter(|_| !locked) {
            match command {
                ViewCommand::Fit => {
                    // Include sections built out on the table, not just the
                    // board and tray.
                    let content = Rect::from_min_size(Pos2::ZERO, content_size);
                    let mut everything = content;
                    for piece in &snapshot.pieces {
                        everything.extend_with(Pos2::new(piece.x, piece.y + tray_layout.board_y));
                        everything.extend_with(Pos2::new(
                            piece.x + snapshot.cell_width(),
                            piece.y + snapshot.cell_height() + tray_layout.board_y,
                        ));
                    }
                    for holder in &snapshot.holders {
                        let area =
                            holder_rect(holder).translate(Vec2::new(0.0, tray_layout.board_y));
                        everything = everything.union(area);
                    }
                    (self.board_zoom, self.board_pan) = if everything == content {
                        (1.0, Vec2::ZERO)
                    } else {
                        focus_content_region(canvas, content_size, everything, base_scale)
                    };
                }
                ViewCommand::Board => {
                    (self.board_zoom, self.board_pan) = focus_content_region(
                        canvas,
                        content_size,
                        Rect::from_min_size(Pos2::new(0.0, tray_layout.board_y), image_size),
                        base_scale,
                    );
                }
                ViewCommand::LoosePieces => {
                    (self.board_zoom, self.board_pan) = focus_content_region(
                        canvas,
                        content_size,
                        Rect::from_min_size(
                            Pos2::new(tray_layout.tray_x, tray_layout.tray_y),
                            Vec2::new(tray_layout.tray_width, tray_layout.tray_height),
                        ),
                        base_scale,
                    );
                }
                ViewCommand::ZoomBy(factor) => {
                    (self.board_zoom, self.board_pan) = zoom_at_pointer(
                        self.board_zoom,
                        self.board_pan,
                        canvas.center(),
                        canvas.center(),
                        factor.ln() / ZOOM_PER_SCROLL_POINT,
                    );
                }
            }
        }
        let scale = base_scale * self.board_zoom;
        let content_origin = canvas.center() + self.board_pan - content_size * (scale * 0.5);
        let board = Rect::from_min_size(
            content_origin + Vec2::new(0.0, tray_layout.board_y * scale),
            image_size * scale,
        );
        let tray = Rect::from_min_size(
            content_origin + Vec2::new(tray_layout.tray_x, tray_layout.tray_y) * scale,
            Vec2::new(tray_layout.tray_width, tray_layout.tray_height) * scale,
        );
        let view = BoardView {
            origin: board.min,
            scale,
        };
        let cw = snapshot.cell_width();
        let ch = snapshot.cell_height();
        let rect_to_screen = |area: Rect| {
            Rect::from_min_max(
                view.to_screen(area.min.x, area.min.y),
                view.to_screen(area.max.x, area.max.y),
            )
        };
        if std::mem::take(&mut self.holders.create_requested) && interactive {
            // Near the pointer when it is over the table, else mid-view.
            let near = response
                .hover_pos()
                .filter(|point| canvas.contains(*point))
                .unwrap_or(canvas.center());
            self.create_holder_near(snapshot, view.to_board(near));
        }

        // The table: everywhere pieces can be put down.
        let scene = snapshot.scene_bounds();
        let table = Rect::from_min_max(
            view.to_screen(scene.min_x, scene.min_y),
            view.to_screen(scene.max_x, scene.max_y),
        );
        painter.rect_filled(canvas, 0.0, Color32::from_rgb(22, 24, 29));
        painter.rect_filled(table, 12.0, Color32::from_rgb(35, 38, 44));
        painter.rect_stroke(
            table,
            12.0,
            Stroke::new(1.0_f32, Color32::from_white_alpha(28)),
            egui::StrokeKind::Inside,
        );
        painter.rect_filled(board.expand(3.0), 6.0, Color32::from_rgb(17, 19, 24));
        painter.rect_stroke(
            board.expand(3.0),
            6.0,
            Stroke::new(1.0_f32, Color32::from_white_alpha(22)),
            egui::StrokeKind::Outside,
        );
        if self.settings.show_ghost {
            // A faint, live ghost of the media marks where the picture goes.
            painter.image(
                texture_id,
                board,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::from_white_alpha(38),
            );
        }
        // Tabs reach past a piece's cell, so pad the tray to contain them.
        painter.rect_filled(
            tray.expand(cw.max(ch) * 0.3 * scale),
            10.0,
            Color32::from_rgb(27, 31, 39),
        );
        // Holders lie on the table, under the pieces.
        let holder_preview = self.holders.drag.as_ref().map(|drag| {
            let area = drag.preview;
            let valid = snapshot
                .check_holder_rect(
                    area.min.x,
                    area.min.y,
                    area.width(),
                    area.height(),
                    Some(drag.holder_id),
                )
                .is_ok();
            (drag.holder_id, area, valid)
        });
        // A replay's copy has no holders; they are not in the log.
        paint_holders(
            &painter,
            &snapshot.holders,
            &self.player_name,
            holder_preview,
            rect_to_screen,
        );

        let order = paint_order(snapshot, &self.selected_pieces);
        let grab_margin = self.settings.mouse.grab_margin;
        let hit_test = |snapshot: &RoomSnapshot,
                        shapes: &std::collections::HashMap<u32, CachedPieceShape>,
                        pointer: Pos2| {
            piece_at(
                snapshot,
                &order,
                shapes,
                self.piece_filter,
                view.to_board(pointer),
                grab_margin / view.scale,
            )
        };
        let pressed_piece = |pointer: Option<Pos2>| {
            pointer.and_then(|pointer| hit_test(snapshot, &self.shape_cache, pointer))
        };
        let idle = interactive
            && self.dragging.is_none()
            && self.pan_drag_last.is_none()
            && self.holders.drag.is_none();
        let grip_at = |pointer: Option<Pos2>| {
            pointer.and_then(|pointer| {
                holder_grip_at(
                    &snapshot.holders,
                    &self.player_name,
                    pointer,
                    rect_to_screen,
                )
            })
        };
        let hovered_grip = if idle {
            grip_at(response.hover_pos())
        } else {
            None
        };
        let hovered_piece = if idle && hovered_grip.is_none() {
            pressed_piece(response.hover_pos())
        } else {
            None
        };
        // Someone else's holder keeps this piece: say so instead of grabbing.
        let reserved_owner = |piece_id: u32| {
            snapshot
                .piece(piece_id)
                .and_then(|piece| snapshot.reserved_from(piece.group_id, Some(&self.player_name)))
                .map(|holder| holder.owner.clone())
        };
        let hovered_reserved = hovered_piece.and_then(reserved_owner);
        let clicked_piece = if interactive && (response.clicked() || response.double_clicked()) {
            pressed_piece(response.interact_pointer_pos())
        } else {
            None
        };
        let drag_start = response.drag_started().then(|| {
            // egui reports the drag only after the pointer has travelled
            // past a threshold, so a fast flick has already left the
            // piece; grab what was under the button press instead.
            let pointer = ui
                .input(|input| input.pointer.press_origin())
                .or_else(|| response.interact_pointer_pos());
            if interactive {
                (pointer, pressed_piece(pointer), grip_at(pointer))
            } else {
                (pointer, None, None)
            }
        });
        if mode == BoardMode::Watch
            && drag_start.is_some_and(|(pointer, ..)| pressed_piece(pointer).is_some())
        {
            self.status = "You are watching: only players can move pieces.".into();
        }

        if response.double_clicked() && clicked_piece.is_none() {
            self.view_command = Some(ViewCommand::Fit);
            ui.ctx().request_repaint();
        }
        if response.clicked() && interactive {
            let select_modifier = self.settings.mouse.select_modifier;
            let control = ui.input(|input| select_modifier.held(&input.modifiers));
            if let Some(id) = clicked_piece {
                if control {
                    if !self.selected_pieces.remove(&id) {
                        self.selected_pieces.insert(id);
                    }
                } else if !self.selected_pieces.contains(&id) {
                    self.selected_pieces.clear();
                    self.selected_pieces.insert(id);
                }
            } else if !control {
                self.selected_pieces.clear();
            }
        }
        if let Some((pointer, piece_id, grip)) = drag_start {
            let select_modifier = self.settings.mouse.select_modifier;
            let (pan_mode, control) = ui.input(|input| {
                (
                    input.modifiers.shift,
                    select_modifier.held(&input.modifiers),
                )
            });
            if pan_mode && locked {
                self.dragging = None;
            } else if pan_mode {
                self.pan_drag_last = response.interact_pointer_pos();
                self.dragging = None;
            } else if let (Some(pointer), Some((holder_id, grip))) = (pointer, grip) {
                if let Some(holder) = snapshot.holders.iter().find(|h| h.id == holder_id) {
                    let origin = holder_rect(holder);
                    self.holders.drag = Some(HolderDrag {
                        holder_id,
                        grip,
                        start: view.to_board(pointer),
                        origin,
                        preview: origin,
                    });
                }
            } else if let (Some(owner), Some(_)) = (piece_id.and_then(reserved_owner), pointer) {
                self.status = format!("That piece is in {owner}'s holder.");
            } else if let (Some(pointer), Some(id)) = (pointer, piece_id) {
                if control {
                    self.selected_pieces.insert(id);
                } else if !self.selected_pieces.contains(&id) {
                    self.selected_pieces.clear();
                    self.selected_pieces.insert(id);
                }
                let mut selected: Vec<_> = self.selected_pieces.iter().copied().collect();
                selected.sort_unstable();
                // Everything joined to a selected piece moves with it, so the
                // local prediction matches the groups the server will move.
                let groups: std::collections::HashSet<u32> = selected
                    .iter()
                    .filter_map(|piece_id| snapshot.piece(*piece_id))
                    .map(|piece| piece.group_id)
                    .collect();
                let reserved = groups.iter().find_map(|group| {
                    snapshot
                        .reserved_from(*group, Some(&self.player_name))
                        .map(|holder| holder.owner.clone())
                });
                let origins = snapshot
                    .pieces
                    .iter()
                    .filter(|piece| !piece.placed && groups.contains(&piece.group_id))
                    .map(|piece| (piece.id, piece.x, piece.y))
                    .collect::<Vec<_>>();
                if let Some(owner) = reserved {
                    // The server would refuse the whole selection.
                    self.status = format!("Part of that selection is in {owner}'s holder.");
                } else if let Some(piece) = snapshot.piece(id).filter(|_| !origins.is_empty()) {
                    self.dragging = Some(ActiveDrag {
                        anchor_piece_id: id,
                        pointer_offset: pointer - view.to_screen(piece.x, piece.y),
                        origins,
                    });
                    self.send(ClientMessage::DragStartSelection {
                        piece_ids: selected,
                    });
                }
            } else if let (true, true, Some(pointer)) = (interactive, control, pointer) {
                // Holding the select modifier on empty space draws a box.
                self.marquee = Some((pointer, pointer));
            } else if pointer.is_some()
                && !locked
                && (self.settings.mouse.drag_empty_pans || !interactive)
            {
                // Dragging empty space pans, like Shift-drag; when nothing
                // on the board can move, any drag pans.
                self.pan_drag_last = response.interact_pointer_pos();
            }
        }
        if response.dragged() {
            if let (Some(drag), Some(pointer)) =
                (self.holders.drag.as_mut(), response.interact_pointer_pos())
            {
                drag.preview = holder_drag_preview(drag, view.to_board(pointer), cw, ch);
            } else if let (Some(marquee), Some(pointer)) =
                (self.marquee.as_mut(), response.interact_pointer_pos())
            {
                marquee.1 = pointer;
            } else if let (Some(last), Some(pointer)) =
                (self.pan_drag_last, response.interact_pointer_pos())
            {
                self.board_pan += pointer - last;
                self.pan_drag_last = Some(pointer);
            } else if let (Some(drag), Some(pointer)) =
                (self.dragging.as_ref(), response.interact_pointer_pos())
            {
                let anchor = drag
                    .origins
                    .iter()
                    .find(|(id, _, _)| *id == drag.anchor_piece_id)
                    .map(|(_, x, y)| (*x, *y))
                    .unwrap_or((0.0, 0.0));
                let target = view.to_board(pointer - drag.pointer_offset);
                // Stop at the table edge exactly where the server will.
                let (dx, dy) = snapshot.clamp_move(
                    drag.origins.iter().map(|(_, x, y)| (*x, *y)),
                    target.x - anchor.0,
                    target.y - anchor.1,
                );
                let target = Pos2::new(anchor.0 + dx, anchor.1 + dy);
                for (id, origin_x, origin_y) in &drag.origins {
                    if let Some(piece) = snapshot.piece_mut(*id) {
                        piece.x = origin_x + dx;
                        piece.y = origin_y + dy;
                        piece.held_by.clone_from(&self.client_id);
                    }
                }
                // Every move rebroadcasts each piece in the dragged groups,
                // which is a lot on a big assembled section. Cap the rate;
                // the release always carries the exact final position.
                let since_last = now - self.last_move_sent_at;
                if since_last >= MOVE_SEND_INTERVAL {
                    self.last_move_sent_at = now;
                    let anchor_piece_id = drag.anchor_piece_id;
                    self.send(ClientMessage::MoveSelection {
                        anchor_piece_id,
                        x: target.x,
                        y: target.y,
                    });
                } else {
                    // Come back for the newest position when the interval is up.
                    ui.ctx().request_repaint_after(Duration::from_secs_f64(
                        MOVE_SEND_INTERVAL - since_last,
                    ));
                }
            }
        }
        if response.drag_stopped()
            && let Some(drag) = self.holders.drag.take()
            && drag.preview != drag.origin
            && let Some(holder) = snapshot
                .holders
                .iter_mut()
                .find(|holder| holder.id == drag.holder_id)
        {
            // Show it in the new spot now; a refusal comes back with a
            // snapshot that puts it back.
            let sent = holder.clone();
            holder.x = drag.preview.min.x;
            holder.y = drag.preview.min.y;
            holder.width = drag.preview.width();
            holder.height = drag.preview.height();
            self.send_holder_update(&sent, drag.preview, sent.label.clone());
        }
        if response.drag_stopped()
            && let Some((start, mut end)) = self.marquee.take()
        {
            if let Some(pointer) = response.interact_pointer_pos() {
                end = pointer;
            }
            let area = Rect::from_two_pos(view.to_board(start), view.to_board(end));
            let hits = pieces_touching(
                snapshot,
                &self.shape_cache,
                self.piece_filter,
                area,
                self.client_id.as_deref(),
            );
            let me = self.player_name.clone();
            self.selected_pieces.extend(hits.into_iter().filter(|id| {
                snapshot.piece(*id).is_some_and(|piece| {
                    snapshot.reserved_from(piece.group_id, Some(&me)).is_none()
                })
            }));
        }
        if response.drag_stopped()
            && self.pan_drag_last.take().is_none()
            && let Some(drag) = self.dragging.take()
        {
            let pointer = response.interact_pointer_pos().or_else(|| {
                snapshot
                    .piece(drag.anchor_piece_id)
                    .map(|piece| view.to_screen(piece.x, piece.y) + drag.pointer_offset)
            });
            if let Some(pointer) = pointer {
                let target = view.to_board(pointer - drag.pointer_offset);
                self.send(ClientMessage::DragEndSelection {
                    anchor_piece_id: drag.anchor_piece_id,
                    x: target.x,
                    y: target.y,
                });
            }
        }
        if let Some(button) = self.settings.mouse.pan_button.pointer_button() {
            let (down, delta) =
                ui.input(|input| (input.pointer.button_down(button), input.pointer.delta()));
            if down
                && !locked
                && self.dragging.is_none()
                && (response.hovered() || self.button_panning)
            {
                self.button_panning = true;
                self.board_pan += delta;
            } else if !down {
                self.button_panning = false;
            }
        } else {
            self.button_panning = false;
        }
        if let Some((start, end)) = self.marquee {
            let area = Rect::from_two_pos(start, end);
            painter.rect_filled(area, 2.0, ACCENT.gamma_multiply(0.18));
            painter.rect_stroke(
                area,
                2.0,
                Stroke::new(1.0_f32, ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        let holder_grip = self
            .holders
            .drag
            .as_ref()
            .map(|drag| drag.grip)
            .or(hovered_grip.map(|(_, grip)| grip));
        if let Some(grip) = holder_grip {
            ui.ctx().set_cursor_icon(match grip {
                HolderGrip::Move => egui::CursorIcon::Move,
                HolderGrip::Resize => egui::CursorIcon::ResizeNwSe,
            });
        } else if self.dragging.is_some() || self.pan_drag_last.is_some() || self.button_panning {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if hovered_reserved.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
        } else if hovered_piece.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }

        // Gestures may have changed what is selected or held.
        let order = paint_order(snapshot, &self.selected_pieces);
        let clip = painter.clip_rect();
        // Each entry: the piece, its mesh, and its on-screen width in points.
        let mut layer_meshes: Vec<(&Piece, Mesh, f32)> = Vec::new();
        let mut cursor = 0;
        while cursor < order.len() {
            let layer = piece_layer(&snapshot.pieces[order[cursor]], &self.selected_pieces);
            layer_meshes.clear();
            while cursor < order.len() {
                let piece = &snapshot.pieces[order[cursor]];
                if piece_layer(piece, &self.selected_pieces) != layer {
                    break;
                }
                cursor += 1;
                if !piece_visible_for_filter(
                    self.piece_filter,
                    piece,
                    snapshot.width,
                    snapshot.rows,
                    snapshot.columns,
                ) {
                    continue;
                }
                let Some(shape) = self.shape_cache.get(&piece.id) else {
                    continue;
                };
                let on_screen = Rect::from_min_max(
                    view.to_screen(piece.x + shape.bounds.min.x, piece.y + shape.bounds.min.y),
                    view.to_screen(piece.x + shape.bounds.max.x, piece.y + shape.bounds.max.y),
                );
                if !clip.intersects(on_screen.expand(12.0)) {
                    continue;
                }
                // A piece far smaller than its full outline is drawn from a
                // thinned one: same shape to the eye, a fraction of the work.
                let Some(detail) = shape.detail(on_screen.width().max(on_screen.height()) * 4.0)
                else {
                    continue;
                };
                let mesh = piece_mesh(
                    texture_id,
                    piece,
                    &detail.triangles,
                    &detail.outline,
                    board,
                    scale,
                    cw,
                    ch,
                    image_size.x,
                    image_size.y,
                    media_uv,
                );
                layer_meshes.push((piece, mesh, on_screen.width()));
            }
            // Shadows for the whole layer go down first so joined neighbours
            // do not shade each other.
            if layer > 0 {
                let (offset, alpha) = if layer == 3 {
                    (Vec2::new(5.0, 8.0), 95)
                } else {
                    (Vec2::new(1.5, 2.5), 75)
                };
                for (_, mesh, width) in &layer_meshes {
                    if *width > 14.0 {
                        painter.add(egui::Shape::mesh(shadow_mesh(mesh, offset, alpha)));
                    }
                }
            }
            for (piece, mesh, width) in layer_meshes.drain(..) {
                let selected = self.selected_pieces.contains(&piece.id);
                // A hairline around a speck is just noise, and on a big
                // board most pieces are specks: leave it out unless the
                // piece is held, selected, or under the pointer.
                let emphasised = selected
                    || piece.held_by.is_some()
                    || hovered_piece == Some(piece.id)
                    || self.snap_flashes.iter().any(|(id, _)| *id == piece.id);
                if width < TINY_PIECE_POINTS && !emphasised {
                    painter.add(egui::Shape::mesh(mesh));
                    continue;
                }
                let border: Vec<Pos2> = mesh.vertices.iter().map(|vertex| vertex.pos).collect();
                painter.add(egui::Shape::mesh(mesh));
                let stroke = if piece.placed {
                    Stroke::new(1.0_f32, Color32::from_black_alpha(70))
                } else if selected || (piece.held_by.is_some() && piece.held_by == self.client_id) {
                    Stroke::new(2.0_f32, ACCENT)
                } else if piece.held_by.is_some() {
                    Stroke::new(2.0_f32, Color32::from_rgb(245, 190, 80))
                } else if hovered_piece == Some(piece.id) {
                    Stroke::new(1.6_f32, Color32::WHITE)
                } else {
                    Stroke::new(1.0_f32, Color32::from_white_alpha(70))
                };
                let flash = self
                    .snap_flashes
                    .iter()
                    .find(|(id, _)| *id == piece.id)
                    .map(|(_, started)| 1.0 - ((now - started) / SNAP_FLASH_SECONDS) as f32);
                if let Some(strength) = flash {
                    painter.add(egui::Shape::closed_line(
                        border.clone(),
                        Stroke::new(
                            1.0 + 3.0 * strength,
                            Color32::from_white_alpha((strength.clamp(0.0, 1.0) * 220.0) as u8),
                        ),
                    ));
                }
                painter.add(egui::Shape::closed_line(border, stroke));
            }
        }
        if !self.snap_flashes.is_empty() {
            ui.ctx().request_repaint();
        }
        if let (Some(owner), Some(pointer)) = (&hovered_reserved, response.hover_pos()) {
            paint_pill(
                &painter,
                pointer + Vec2::new(0.0, 22.0),
                &format!("🔒 In {owner}'s holder"),
                Color32::WHITE,
            );
        }

        // Live cursors: ours goes out while it is over the table and this
        // window has focus; everyone else's is drawn above the pieces.
        let (hover, focused) = ui.input(|input| (input.pointer.hover_pos(), input.focused));
        let on_table = hover
            .filter(|point| {
                focused
                    && canvas.contains(*point)
                    && (response.contains_pointer() || response.dragged())
            })
            .map(|point| view.to_board(point))
            .filter(|point| scene.contains(point.x, point.y));
        // Only players point and ping; a replay shows none of the live room.
        let pointing = on_table.filter(|_| interactive);
        self.share_pointer(pointing, now, ui.ctx());
        if interactive {
            self.maybe_ping(on_table);
        } else if std::mem::take(&mut self.chat.ping_requested) && mode == BoardMode::Watch {
            self.status = "Spectators can chat but cannot ping the board.".into();
        }
        if mode != BoardMode::Replay {
            self.paint_pings(
                &painter,
                canvas,
                |point| view.to_screen(point.x, point.y),
                now,
                ui.ctx(),
            );
            self.paint_remote_cursors(
                &painter,
                |point| view.to_screen(point.x, point.y),
                now,
                ui.ctx(),
            );
        }

        let total = snapshot.pieces.len();
        let placed = snapshot.pieces.iter().filter(|piece| piece.placed).count();
        let mut group_sizes = std::collections::HashMap::<u32, usize>::new();
        for piece in &snapshot.pieces {
            *group_sizes.entry(piece.group_id).or_default() += 1;
        }
        // A lone piece is not progress; show the biggest joined cluster.
        let largest = group_sizes.values().copied().max().unwrap_or(0);
        let joined = if largest > 1 {
            largest.max(placed)
        } else {
            placed
        };
        let completed = snapshot.completed();
        let solve_time = match (mode, self.replay.as_ref()) {
            (BoardMode::Replay, Some(replay)) => Some(replay.solve_time()),
            _ => Self::solve_time(snapshot),
        };
        let shown_time = if self.settings.hide_solve_timer {
            None
        } else {
            solve_time
        };
        paint_progress(&painter, canvas, joined, total, shown_time);
        if shown_time.is_some() && snapshot.stats.running() {
            // Tick on the next whole second; no clock messages are needed.
            let into_second = shown_time.unwrap_or_default().subsec_millis();
            ui.ctx().request_repaint_after(Duration::from_millis(
                u64::from(1_000 - into_second).max(50),
            ));
        }
        let banner = match mode {
            BoardMode::Replay => Some(("Timelapse replay · the live room carries on", ACCENT)),
            BoardMode::Watch => Some(("Watching · only players move pieces", ACCENT)),
            BoardMode::Play if self.still_assist_texture.is_some() => Some((
                "Still-frame assist · playback continues",
                Color32::LIGHT_YELLOW,
            )),
            BoardMode::Play => None,
        };
        if let Some((text, color)) = banner {
            paint_pill(
                &painter,
                canvas.center_top() + Vec2::new(0.0, 12.0),
                text,
                color,
            );
        }
        if mode == BoardMode::Replay {
            return None;
        }
        if !completed {
            self.win_dismissed_room = None;
        }
        let mut action = None;
        if completed && self.win_dismissed_room.as_deref() != Some(snapshot.id.as_str()) {
            let time = ui.input(|input| input.time) as f32;
            paint_confetti(&painter, canvas, time);
            let lines = completion_lines(snapshot, solve_time);
            let card = Rect::from_min_size(
                canvas.center_top() + Vec2::new(-190.0, 25.0),
                Vec2::new(
                    380.0,
                    130.0 + COMPLETION_ROW_HEIGHT * lines.players.len() as f32,
                ),
            );
            painter.rect_filled(card, 14.0, Color32::from_black_alpha(215));
            painter.rect_stroke(
                card,
                14.0,
                Stroke::new(1.0_f32, Color32::from_white_alpha(40)),
                egui::StrokeKind::Inside,
            );
            painter.text(
                card.center_top() + Vec2::new(0.0, 36.0),
                egui::Align2::CENTER_CENTER,
                "Puzzle complete!",
                egui::FontId::proportional(34.0),
                Color32::from_rgb(120, 230, 150),
            );
            painter.text(
                card.center_top() + Vec2::new(0.0, 68.0),
                egui::Align2::CENTER_CENTER,
                &lines.summary,
                egui::FontId::proportional(15.0),
                Color32::LIGHT_GRAY,
            );
            for (index, (name, pieces, color)) in lines.players.iter().enumerate() {
                let y = card.top() + 96.0 + COMPLETION_ROW_HEIGHT * index as f32;
                let left = card.left() + 70.0;
                let right = card.right() - 70.0;
                painter.circle_filled(Pos2::new(left, y), 4.5, *color);
                painter.text(
                    Pos2::new(left + 12.0, y),
                    egui::Align2::LEFT_CENTER,
                    name,
                    egui::FontId::proportional(14.0),
                    *color,
                );
                painter.text(
                    Pos2::new(right, y),
                    egui::Align2::RIGHT_CENTER,
                    pieces,
                    egui::FontId::proportional(14.0),
                    Color32::LIGHT_GRAY,
                );
            }
            let is_host = self.client_id.as_deref() == Some(snapshot.host_id.as_str());
            let row_y = card.center_bottom().y - 24.0;
            let buttons = if is_host { 3.0 } else { 2.0 };
            let button = |index: f32| {
                let x = card.center().x + (index - (buttons - 1.0) / 2.0) * 118.0;
                Rect::from_center_size(Pos2::new(x, row_y), Vec2::new(110.0, 28.0))
            };
            if ui
                .put(button(0.0), egui::Button::new("Keep watching"))
                .clicked()
            {
                self.win_dismissed_room = Some(snapshot.id.clone());
            }
            if ui
                .put(button(1.0), egui::Button::new("Timelapse"))
                .on_hover_text("Replay the whole solve in under a minute")
                .clicked()
            {
                self.win_dismissed_room = Some(snapshot.id.clone());
                action = Some(BoardAction::WatchTimelapse);
            }
            if is_host
                && ui
                    .put(button(2.0), egui::Button::new("Play again"))
                    .clicked()
            {
                action = Some(BoardAction::PlayAgain);
            }
            ui.ctx().request_repaint();
        }
        action
    }
}

/// Height of one player row on the completion card.
const COMPLETION_ROW_HEIGHT: f32 = 22.0;

/// Most players listed by name on the completion card.
const COMPLETION_MAX_ROWS: usize = 8;

/// What the completion card says: the total, then who placed what.
pub(crate) struct CompletionLines {
    pub(crate) summary: String,
    /// Name, piece count text and colour, most pieces first.
    pub(crate) players: Vec<(String, String, Color32)>,
}

pub(crate) fn completion_lines(
    snapshot: &RoomSnapshot,
    solve_time: Option<Duration>,
) -> CompletionLines {
    let total = snapshot.pieces.len();
    let summary = match solve_time {
        Some(elapsed) => format!("All {total} pieces joined in {}", format_elapsed(elapsed)),
        None => format!("All {total} pieces joined"),
    };
    let ranking = snapshot.stats.ranking();
    let count = |pieces: u32| {
        if pieces == 1 {
            "1 piece".to_owned()
        } else {
            format!("{pieces} pieces")
        }
    };
    let mut players: Vec<_> = ranking
        .iter()
        .take(COMPLETION_MAX_ROWS)
        .map(|player| {
            (
                player.name.clone(),
                count(player.pieces),
                player_color(&player.name),
            )
        })
        .collect();
    if ranking.len() > COMPLETION_MAX_ROWS {
        let rest = &ranking[COMPLETION_MAX_ROWS..];
        players.push((
            format!("{} more", rest.len()),
            count(rest.iter().map(|player| player.pieces).sum()),
            Color32::GRAY,
        ));
    }
    CompletionLines { summary, players }
}

/// The local wall clock in milliseconds since the Unix epoch, the scale the
/// server stamps the solve clock with.
/// How far the server's wall clock is ahead of ours, learned at each welcome.
static SERVER_CLOCK_OFFSET_MS: AtomicI64 = AtomicI64::new(0);

/// Remember the server's clock from a welcome; zero means it did not say.
pub(crate) fn set_server_clock(server_now_ms: u64) {
    if server_now_ms > 0 {
        let offset = server_now_ms as i64 - wall_clock_ms() as i64;
        SERVER_CLOCK_OFFSET_MS.store(offset, Ordering::Relaxed);
    }
}

/// Our best guess at the server's wall clock now. Use it for anything the
/// server stamped (the solve clock, chat times) so a client whose own clock
/// is off still shows the right numbers.
pub(crate) fn server_clock_ms() -> u64 {
    (wall_clock_ms() as i64)
        .saturating_add(SERVER_CLOCK_OFFSET_MS.load(Ordering::Relaxed))
        .max(0) as u64
}

pub(crate) fn wall_clock_ms() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// A flat, untextured copy of a piece mesh used as its drop shadow.
fn shadow_mesh(mesh: &Mesh, offset: Vec2, alpha: u8) -> Mesh {
    Mesh {
        indices: mesh.indices.clone(),
        vertices: mesh
            .vertices
            .iter()
            .map(|vertex| Vertex {
                pos: vertex.pos + offset,
                uv: egui::epaint::WHITE_UV,
                color: Color32::from_black_alpha(alpha),
            })
            .collect(),
        texture_id: egui::TextureId::default(),
    }
}

/// A small rounded caption centred on `center_top`.
fn paint_pill(painter: &egui::Painter, center_top: Pos2, text: &str, color: Color32) {
    let galley = painter.layout_no_wrap(text.to_owned(), egui::FontId::proportional(13.0), color);
    let rect = Rect::from_min_size(
        center_top - Vec2::new(galley.size().x / 2.0 + 12.0, 0.0),
        galley.size() + Vec2::new(24.0, 10.0),
    );
    painter.rect_filled(rect, rect.height() / 2.0, Color32::from_black_alpha(170));
    painter.galley(rect.min + Vec2::new(12.0, 5.0), galley, color);
}

/// Progress readout in the board's top-left corner.
fn paint_progress(
    painter: &egui::Painter,
    canvas: Rect,
    joined: usize,
    total: usize,
    solve_time: Option<Duration>,
) {
    let card = Rect::from_min_size(
        canvas.left_top() + Vec2::splat(12.0),
        Vec2::new(210.0, 46.0),
    );
    painter.rect_filled(card, 10.0, Color32::from_black_alpha(170));
    painter.text(
        card.left_top() + Vec2::new(12.0, 8.0),
        egui::Align2::LEFT_TOP,
        format!("{joined}/{total} joined"),
        egui::FontId::proportional(15.0),
        Color32::WHITE,
    );
    if let Some(elapsed) = solve_time {
        painter.text(
            card.right_top() + Vec2::new(-12.0, 9.0),
            egui::Align2::RIGHT_TOP,
            format_elapsed(elapsed),
            egui::FontId::monospace(13.0),
            Color32::LIGHT_GRAY,
        );
    }
    let track = Rect::from_min_max(
        card.left_bottom() + Vec2::new(12.0, -13.0),
        card.right_bottom() + Vec2::new(-12.0, -8.0),
    );
    painter.rect_filled(track, 2.5, Color32::from_white_alpha(28));
    if total > 0 && joined > 0 {
        let mut fill = track;
        fill.max.x = track.min.x + track.width() * (joined as f32 / total as f32).min(1.0);
        painter.rect_filled(fill, 2.5, SUCCESS);
    }
}

/// Deterministic falling confetti; `time` drives the animation.
pub(crate) fn paint_confetti(painter: &egui::Painter, area: Rect, time: f32) {
    const COLORS: [Color32; 5] = [
        Color32::from_rgb(255, 99, 99),
        Color32::from_rgb(255, 205, 86),
        Color32::from_rgb(99, 220, 140),
        Color32::from_rgb(90, 170, 255),
        Color32::from_rgb(200, 120, 255),
    ];
    for index in 0..90_u32 {
        let hash = index.wrapping_mul(2_654_435_761).rotate_left(7) ^ 0x9e37_79b9;
        let unit = |shift: u32| ((hash >> shift) & 0xff) as f32 / 255.0;
        let speed = 0.12 + unit(0) * 0.18;
        let fall = (time * speed + unit(8)).fract();
        let sway = (time * (1.0 + unit(16) * 2.0) + unit(24) * 6.0).sin() * 14.0;
        let position = Pos2::new(
            area.left() + unit(16) * area.width() + sway,
            area.top() + fall * (area.height() + 20.0) - 10.0,
        );
        painter.rect_filled(
            Rect::from_center_size(position, Vec2::new(7.0, 4.0 + unit(8) * 6.0)),
            1.0,
            COLORS[(index % 5) as usize],
        );
    }
}

pub(crate) fn fit_media_rect(bounds: Rect, media_size: Vec2) -> Rect {
    if media_size.x <= 0.0 || media_size.y <= 0.0 {
        return bounds;
    }
    let scale = (bounds.width() / media_size.x).min(bounds.height() / media_size.y);
    Rect::from_center_size(bounds.center(), media_size * scale)
}

pub(crate) const MIN_ZOOM: f32 = 0.05;
pub(crate) const MAX_ZOOM: f32 = 8.0;
const ZOOM_PER_SCROLL_POINT: f32 = 0.002;

pub(crate) fn zoom_at_pointer(
    current_zoom: f32,
    current_pan: Vec2,
    canvas_center: Pos2,
    pointer: Pos2,
    scroll_y: f32,
) -> (f32, Vec2) {
    let current_zoom = current_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    let world_offset = (pointer - (canvas_center + current_pan)) / current_zoom;
    let zoom = (current_zoom * (scroll_y * ZOOM_PER_SCROLL_POINT).exp()).clamp(MIN_ZOOM, MAX_ZOOM);
    let pan = pointer - canvas_center - world_offset * zoom;
    (zoom, pan)
}

pub(crate) fn focus_content_region(
    canvas: Rect,
    content_size: Vec2,
    region: Rect,
    base_scale: f32,
) -> (f32, Vec2) {
    let target_scale = (canvas.width() * 0.9 / region.width())
        .min(canvas.height() * 0.9 / region.height())
        .clamp(base_scale * 0.05, base_scale * 20.0);
    let zoom = target_scale / base_scale;
    let content_center = content_size * 0.5;
    let region_center = region.center().to_vec2();
    let pan = (content_center - region_center) * target_scale;
    (zoom, pan)
}

/// UV sub-rectangle that makes media of the given size cover a board of the
/// given size (centre crop, no stretch). Unknown sizes sample everything.
pub(crate) fn cover_uv_rect(board_w: f32, board_h: f32, media_w: f32, media_h: f32) -> Rect {
    let full = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    if board_w <= 0.0 || board_h <= 0.0 || media_w <= 0.0 || media_h <= 0.0 {
        return full;
    }
    let board_aspect = board_w / board_h;
    let media_aspect = media_w / media_h;
    if (board_aspect - media_aspect).abs() < 1e-4 {
        full
    } else if media_aspect > board_aspect {
        let inset = (1.0 - board_aspect / media_aspect) * 0.5;
        Rect::from_min_max(Pos2::new(inset, 0.0), Pos2::new(1.0 - inset, 1.0))
    } else {
        let inset = (1.0 - media_aspect / board_aspect) * 0.5;
        Rect::from_min_max(Pos2::new(0.0, inset), Pos2::new(1.0, 1.0 - inset))
    }
}

// A mesh vertex needs source, destination, and grid geometry together.
#[allow(clippy::too_many_arguments)]
pub(crate) fn piece_mesh(
    texture: egui::TextureId,
    piece: &Piece,
    triangles: &[[usize; 3]],
    outline: &[[f32; 2]],
    board: Rect,
    scale: f32,
    cell_w: f32,
    cell_h: f32,
    image_w: f32,
    image_h: f32,
    uv: Rect,
) -> Mesh {
    let mut mesh = Mesh::with_texture(texture);
    mesh.vertices.reserve(outline.len());
    for point in outline {
        let source_x = piece.column as f32 * cell_w + point[0];
        let source_y = piece.row as f32 * cell_h + point[1];
        mesh.vertices.push(Vertex {
            pos: Pos2::new(
                board.min.x + (piece.x + point[0]) * scale,
                board.min.y + (piece.y + point[1]) * scale,
            ),
            uv: Pos2::new(
                uv.min.x + (source_x / image_w) * (uv.max.x - uv.min.x),
                uv.min.y + (source_y / image_h) * (uv.max.y - uv.min.y),
            ),
            color: Color32::WHITE,
        });
    }
    for [a, b, c] in triangles {
        mesh.add_triangle(*a as u32, *b as u32, *c as u32);
    }
    mesh
}

pub(crate) fn triangulate_polygon(points: &[[f32; 2]]) -> Option<Vec<[usize; 3]>> {
    if points.len() < 3 {
        return None;
    }
    let signed_area = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum::<f32>();
    if signed_area.abs() < f32::EPSILON {
        return None;
    }
    let winding = signed_area.signum();
    let mut remaining = (0..points.len()).collect::<Vec<_>>();
    let mut triangles = Vec::with_capacity(points.len() - 2);
    let mut attempts = 0;
    while remaining.len() > 3 && attempts < points.len() * points.len() {
        attempts += 1;
        let mut found = false;
        for index in 0..remaining.len() {
            let a = remaining[(index + remaining.len() - 1) % remaining.len()];
            let b = remaining[index];
            let c = remaining[(index + 1) % remaining.len()];
            if cross(points[a], points[b], points[c]) * winding <= 1e-5 {
                continue;
            }
            if remaining.iter().copied().any(|candidate| {
                candidate != a
                    && candidate != b
                    && candidate != c
                    && point_in_triangle(
                        points[candidate],
                        points[a],
                        points[b],
                        points[c],
                        winding,
                    )
            }) {
                continue;
            }
            triangles.push([a, b, c]);
            remaining.remove(index);
            found = true;
            break;
        }
        if !found {
            return None;
        }
    }
    if remaining.len() == 3 {
        triangles.push([remaining[0], remaining[1], remaining[2]]);
        Some(triangles)
    } else {
        None
    }
}

pub(crate) fn cross(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

pub(crate) fn point_in_triangle(
    p: [f32; 2],
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    winding: f32,
) -> bool {
    cross(a, b, p) * winding >= -1e-5
        && cross(b, c, p) * winding >= -1e-5
        && cross(c, a, p) * winding >= -1e-5
}

pub(crate) fn point_in_polygon(point: Pos2, polygon: &[[f32; 2]]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut previous = polygon.len() - 1;
    for current in 0..polygon.len() {
        let a = polygon[current];
        let b = polygon[previous];
        if (a[1] > point.y) != (b[1] > point.y)
            && point.x < (b[0] - a[0]) * (point.y - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        previous = current;
    }
    inside
}
