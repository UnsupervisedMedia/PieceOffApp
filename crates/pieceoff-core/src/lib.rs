//! Shared puzzle rules and versioned messages. This crate stays independent of UI,
//! transport, and media decoding so desktop, web, and mobile clients can share it.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

mod apply;
mod membership;
pub use apply::ApplyOutcome;
pub use membership::{JoinOutcome, LeaveOutcome, MAX_PLAYERS, UploadAssignment};
mod sanitize;
pub use sanitize::{clean_media_extension, clean_name, clean_title, valid_sha256};
mod claims;
pub use claims::{
    MAX_NAME_CLAIMS, MAX_PLAYER_KEY_BYTES, MAX_ROOM_PLAYERS, NAME_TAKEN, NameClaim,
    REPLACED_BY_NEW_CONNECTION,
};
mod holders;
mod stats;
mod timelapse;
pub use holders::{
    HOLDER_DEFAULT_CELLS, HOLDER_GRACE_MS, HOLDER_MAX_CELLS, HOLDER_MIN_CELLS,
    MAX_HOLDER_LABEL_CHARS, MAX_HOLDERS_PER_PLAYER, MAX_HOLDERS_PER_ROOM, PieceHolder,
    clean_holder_label,
};
pub use stats::{MAX_SOLVE_PLAYERS, PlayerSolveStats, SolveStats};
pub use timelapse::{
    MAX_ROOM_SPECTATORS, MAX_TIMELAPSE_POSITIONS, StoredRoom, Timelapse, TimelapseFrame,
};

pub const PROTOCOL_VERSION: u16 = 13;
/// Longest chat line, in characters, after the server tidies it.
pub const MAX_CHAT_CHARS: usize = 300;
pub const MAX_QUEUE_ITEMS: usize = 64;
pub const MAX_MEDIA_DIMENSION: u32 = 8192;
pub const MAX_MEDIA_PIXELS: u64 = 40_000_000;

pub const SUPPORTED_MEDIA_EXTENSIONS: &[&str] = &[
    "png", "apng", "jpg", "jpeg", "webp", "gif", "mp4", "m4v", "mov", "mkv", "webm", "avi", "mpg",
    "mpeg", "ogv", "flv", "3gp", "wmv", "ts", "mts", "m2ts",
];

pub fn valid_media_extension(extension: &str) -> Option<&'static str> {
    SUPPORTED_MEDIA_EXTENSIONS
        .iter()
        .copied()
        .find(|supported| *supported == extension)
}

pub fn valid_media_dimensions(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= MAX_MEDIA_DIMENSION
        && height <= MAX_MEDIA_DIMENSION
        && u64::from(width) * u64::from(height) <= MAX_MEDIA_PIXELS
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct PuzzleId(pub String);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Piece {
    pub id: u32,
    pub group_id: u32,
    pub row: u16,
    pub column: u16,
    /// Piece position in source-image pixels. The board has a clear target at
    /// (column * cell_width, row * cell_height).
    pub x: f32,
    pub y: f32,
    pub placed: bool,
    pub held_by: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaybackState {
    pub playing: bool,
    pub position_ms: u64,
    pub rate: f32,
    pub loop_start_ms: u64,
    pub loop_end_ms: u64,
    /// Unix timestamp at which `position_ms` was authoritative.
    pub changed_at_ms: u64,
}

impl PlaybackState {
    /// Position at `now_ms` without applying loop wrapping.
    pub fn position_at(&self, now_ms: u64) -> u64 {
        if !self.playing {
            return self.position_ms;
        }
        let elapsed = now_ms.saturating_sub(self.changed_at_ms) as f64 * self.rate as f64;
        self.position_ms.saturating_add(elapsed as u64)
    }
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self {
            playing: false,
            position_ms: 0,
            rate: 1.0,
            loop_start_ms: 0,
            loop_end_ms: 0,
            changed_at_ms: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaItem {
    pub sha256: String,
    #[serde(default = "default_media_extension")]
    pub extension: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub available: bool,
    /// Last position, loop in and loop out while this item was current.
    #[serde(default)]
    pub position_ms: u64,
    #[serde(default)]
    pub loop_start_ms: u64,
    #[serde(default)]
    pub loop_end_ms: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatMode {
    Off,
    #[default]
    One,
    All,
}

pub fn fnv1a(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5u32, |hash, byte| {
        (hash ^ byte as u32).wrapping_mul(0x0100_0193)
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoomSnapshot {
    pub protocol_version: u16,
    pub id: String,
    pub invite_code: String,
    pub title: String,
    pub media_sha256: String,
    #[serde(default = "default_media_extension")]
    pub media_extension: String,
    #[serde(default)]
    pub media_available: bool,
    pub width: u32,
    pub height: u32,
    pub rows: u16,
    pub columns: u16,
    pub pieces: Vec<Piece>,
    pub playback: PlaybackState,
    pub host_id: String,
    pub revision: u64,
    #[serde(default)]
    pub shape_seed: u32,
    #[serde(default)]
    pub queue: Vec<MediaItem>,
    #[serde(default)]
    pub current: usize,
    #[serde(default)]
    pub repeat: RepeatMode,
    #[serde(default)]
    pub video_lock: bool,
    /// The solve clock and per-player credits (protocol v11). Older
    /// snapshots load with a clock at zero that starts on the next pickup.
    #[serde(default)]
    pub stats: SolveStats,
    /// Players' private sorting areas (protocol v12). Older snapshots load
    /// with none.
    #[serde(default)]
    pub holders: Vec<PieceHolder>,
}

pub const PIECE_TRAY_SPACING_CELLS: f32 = 1.75;
const PIECE_TRAY_GUTTER_CELLS: f32 = 0.7;
const PIECE_TRAY_INSET_CELLS: f32 = 0.2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PieceTrayLayout {
    pub columns: usize,
    pub rows: usize,
    pub tray_x: f32,
    pub tray_y: f32,
    pub tray_width: f32,
    pub tray_height: f32,
    pub board_y: f32,
    pub content_width: f32,
    pub content_height: f32,
}

pub fn piece_tray_layout(width: u32, height: u32, rows: u16, columns: u16) -> PieceTrayLayout {
    let rows = rows.max(1) as f32;
    let columns = columns.max(1) as f32;
    let width = width as f32;
    let height = height as f32;
    let cell_width = width / columns;
    let cell_height = height / rows;
    let total_pieces = (rows * columns) as usize;
    let tray_columns = ((columns as usize * 3) / 5).max(1);
    let tray_rows = total_pieces.div_ceil(tray_columns);
    let tray_width = tray_columns as f32 * PIECE_TRAY_SPACING_CELLS * cell_width;
    let tray_height = tray_rows as f32 * PIECE_TRAY_SPACING_CELLS * cell_height;
    let content_height = height.max(tray_height);
    let board_y = (content_height - height) * 0.5;
    let tray_y = (content_height - tray_height) * 0.5 + PIECE_TRAY_INSET_CELLS * cell_height;
    let tray_gutter = PIECE_TRAY_GUTTER_CELLS * cell_width;
    let tray_x = width + tray_gutter;
    PieceTrayLayout {
        columns: tray_columns,
        rows: tray_rows,
        tray_x,
        tray_y,
        tray_width,
        tray_height,
        board_y,
        content_width: tray_x + tray_width + tray_gutter,
        content_height,
    }
}

fn clamp_group_delta(
    delta: f32,
    group_min: f32,
    group_max: f32,
    scene_min: f32,
    scene_max: f32,
) -> f32 {
    let min_delta = scene_min - group_min;
    let max_delta = scene_max - group_max;
    if min_delta <= max_delta {
        delta.clamp(min_delta, max_delta)
    } else {
        0.0
    }
}

/// Free table space on every side of the board and tray, in multiples of
/// their combined size, so sections can be assembled well away from both.
const SCENE_MARGIN_FACTOR: f32 = 3.0;

/// The table area pieces may occupy, in source-image pixels relative to the
/// board's top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneBounds {
    pub min_x: f32,
    pub max_x: f32,
    pub min_y: f32,
    pub max_y: f32,
}

impl SceneBounds {
    /// Whether a point, such as a player's pointer, is a real spot on the
    /// table: finite and inside the bounds.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x.is_finite()
            && y.is_finite()
            && (self.min_x..=self.max_x).contains(&x)
            && (self.min_y..=self.max_y).contains(&y)
    }
}

impl RoomSnapshot {
    pub fn scene_bounds(&self) -> SceneBounds {
        let tray = piece_tray_layout(self.width, self.height, self.rows, self.columns);
        let margin_x = tray.content_width * SCENE_MARGIN_FACTOR;
        let margin_y = tray.content_height * SCENE_MARGIN_FACTOR;
        SceneBounds {
            min_x: -margin_x,
            max_x: tray.content_width + margin_x,
            min_y: -tray.board_y - margin_y,
            max_y: tray.content_height - tray.board_y + margin_y,
        }
    }

    /// Limit a rigid move of the pieces at `positions` so every one of them
    /// stays on the table. The server applies this to each move; clients use
    /// it too so an optimistic drag stops at the same edge.
    pub fn clamp_move(
        &self,
        positions: impl IntoIterator<Item = (f32, f32)>,
        dx: f32,
        dy: f32,
    ) -> (f32, f32) {
        let cell_w = self.cell_width();
        let cell_h = self.cell_height();
        let (mut min_x, mut max_x) = (f32::INFINITY, f32::NEG_INFINITY);
        let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
        for (x, y) in positions {
            min_x = min_x.min(x);
            max_x = max_x.max(x + cell_w);
            min_y = min_y.min(y);
            max_y = max_y.max(y + cell_h);
        }
        let scene = self.scene_bounds();
        (
            clamp_group_delta(dx, min_x, max_x, scene.min_x, scene.max_x),
            clamp_group_delta(dy, min_y, max_y, scene.min_y, scene.max_y),
        )
    }
}

fn default_media_extension() -> String {
    "bin".into()
}

impl RoomSnapshot {
    pub fn cell_width(&self) -> f32 {
        self.width as f32 / self.columns as f32
    }

    pub fn cell_height(&self) -> f32 {
        self.height as f32 / self.rows as f32
    }

    pub fn piece(&self, id: u32) -> Option<&Piece> {
        self.pieces.iter().find(|piece| piece.id == id)
    }

    pub fn piece_mut(&mut self, id: u32) -> Option<&mut Piece> {
        self.pieces.iter_mut().find(|piece| piece.id == id)
    }

    /// A puzzle is finished once every piece is snapped to the board or all
    /// pieces have been joined into a single group (assembled anywhere).
    pub fn completed(&self) -> bool {
        let Some(first) = self.pieces.first() else {
            return false;
        };
        self.pieces.iter().all(|piece| piece.placed)
            || self
                .pieces
                .iter()
                .all(|piece| piece.group_id == first.group_id)
    }

    /// How much of the puzzle is assembled, 0 to 100: pieces that are snapped
    /// to the board or joined to a neighbour count as settled.
    pub fn progress_percent(&self) -> u8 {
        if self.pieces.is_empty() {
            return 0;
        }
        if self.completed() {
            return 100;
        }
        let settled = self.settled_pieces();
        // Never report 100 until `completed()` says so.
        ((settled * 100) / self.pieces.len()).min(99) as u8
    }

    pub fn effective_shape_seed(&self) -> u32 {
        if self.shape_seed != 0 {
            return self.shape_seed;
        }
        let hash = self
            .queue
            .first()
            .map_or(self.media_sha256.as_str(), |item| item.sha256.as_str());
        fnv1a(hash)
    }

    pub fn current_item(&self) -> Option<&MediaItem> {
        self.queue.get(self.current)
    }

    /// Keep the legacy single-media fields equal to the current queue item.
    pub fn sync_current_mirror(&mut self) {
        if let Some(item) = self.queue.get(self.current) {
            self.media_sha256 = item.sha256.clone();
            self.media_extension = item.extension.clone();
            self.media_available = item.available;
        }
    }

    /// Bring a snapshot saved by an older protocol up to the current model.
    pub fn migrate(&mut self) {
        if self.queue.is_empty() && !self.media_sha256.is_empty() {
            self.queue.push(MediaItem {
                sha256: self.media_sha256.clone(),
                extension: self.media_extension.clone(),
                title: self.title.clone(),
                width: self.width,
                height: self.height,
                duration_ms: 0,
                available: self.media_available,
                position_ms: self.playback.position_ms,
                loop_start_ms: self.playback.loop_start_ms,
                loop_end_ms: self.playback.loop_end_ms,
            });
            self.current = 0;
            self.repeat = RepeatMode::One;
        }
        if self.shape_seed == 0 {
            self.shape_seed = self.effective_shape_seed();
        }
        self.current = self.current.min(self.queue.len().saturating_sub(1));
        // A loaded room has nobody in it yet; its host resumes the clock when
        // a player is present.
        self.stats.running_since_ms = None;
        self.protocol_version = PROTOCOL_VERSION;
        self.sync_current_mirror();
    }

    // Keep room construction explicit at the protocol boundary; these fields
    // are the complete immutable puzzle definition sent to every client.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        id: String,
        invite_code: String,
        title: String,
        media_sha256: String,
        media_extension: String,
        width: u32,
        height: u32,
        rows: u16,
        columns: u16,
        host_id: String,
    ) -> Result<Self, String> {
        if !valid_media_dimensions(width, height) {
            return Err(format!(
                "Media dimensions must be at most {MAX_MEDIA_DIMENSION} pixels per side and {MAX_MEDIA_PIXELS} pixels total"
            ));
        }
        if !(2..=30).contains(&rows) || !(2..=30).contains(&columns) {
            return Err("Choose between 2 and 30 rows and columns".into());
        }
        if rows as u32 * columns as u32 > 400 {
            return Err("A room can contain at most 400 pieces".into());
        }

        let pieces = scattered_pieces(
            &id,
            &invite_code,
            &media_sha256,
            width,
            height,
            rows,
            columns,
            0,
        );

        let queue = vec![MediaItem {
            sha256: media_sha256.clone(),
            extension: media_extension.clone(),
            title: title.clone(),
            width,
            height,
            duration_ms: 0,
            available: false,
            position_ms: 0,
            loop_start_ms: 0,
            loop_end_ms: 0,
        }];
        let shape_seed = fnv1a(&media_sha256);
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            id,
            invite_code,
            title,
            media_sha256,
            media_extension,
            media_available: false,
            width,
            height,
            rows,
            columns,
            pieces,
            playback: PlaybackState::default(),
            host_id,
            revision: 0,
            shape_seed,
            queue,
            current: 0,
            repeat: RepeatMode::One,
            video_lock: false,
            stats: SolveStats::default(),
            holders: Vec::new(),
        })
    }

    /// Rehost a portable local puzzle snapshot in a new room. Network identity,
    /// upload state, playback activity, and drag leases are always regenerated.
    pub fn rehost(
        mut self,
        id: String,
        invite_code: String,
        host_id: String,
    ) -> Result<Self, String> {
        self.migrate();
        if self.queue.len() > MAX_QUEUE_ITEMS
            || self.queue.iter().any(|item| {
                !valid_media_fingerprint(&item.sha256)
                    || valid_media_extension(&item.extension).is_none()
                    || !valid_media_dimensions(item.width, item.height)
            })
        {
            return Err("Puzzle snapshot has an invalid media queue".into());
        }
        if !valid_media_dimensions(self.width, self.height) {
            return Err(format!(
                "Media dimensions must be at most {MAX_MEDIA_DIMENSION} pixels per side and {MAX_MEDIA_PIXELS} pixels total"
            ));
        }
        if !(2..=30).contains(&self.rows)
            || !(2..=30).contains(&self.columns)
            || self.rows as u32 * self.columns as u32 > 400
        {
            return Err("Puzzle grid must contain between 4 and 400 pieces".into());
        }
        let piece_count = self.rows as usize * self.columns as usize;
        if self.pieces.len() != piece_count {
            return Err("Puzzle snapshot has an invalid piece count".into());
        }
        let SceneBounds {
            min_x,
            max_x,
            min_y,
            max_y,
        } = self.scene_bounds();
        let mut seen = HashSet::with_capacity(piece_count);
        for piece in &mut self.pieces {
            let expected_row = (piece.id / self.columns as u32) as u16;
            let expected_column = (piece.id % self.columns as u32) as u16;
            if piece.id as usize >= piece_count
                || !seen.insert(piece.id)
                || piece.row != expected_row
                || piece.column != expected_column
                || piece.group_id as usize >= piece_count
                || !piece.x.is_finite()
                || !piece.y.is_finite()
                || piece.x < min_x
                || piece.x > max_x
                || piece.y < min_y
                || piece.y > max_y
            {
                return Err("Puzzle snapshot contains invalid piece state".into());
            }
            piece.held_by = None;
        }
        if !valid_media_fingerprint(&self.media_sha256) {
            return Err("Puzzle snapshot has an invalid media fingerprint".into());
        }
        if !self.playback.rate.is_finite() {
            return Err("Puzzle snapshot has an invalid playback rate".into());
        }
        if self.playback.loop_end_ms < self.playback.loop_start_ms {
            return Err("Puzzle snapshot has an invalid playback loop".into());
        }
        self.stats.sanitize();
        // Holders belong to the players of the room they were made in.
        self.holders.clear();
        self.id = id;
        self.invite_code = invite_code;
        self.host_id = host_id;
        self.protocol_version = PROTOCOL_VERSION;
        self.media_available = false;
        for item in &mut self.queue {
            item.available = false;
        }
        self.sync_current_mirror();
        self.revision = 1;
        self.playback.playing = false;
        self.playback.rate = self.playback.rate.clamp(0.25, 2.0);
        self.playback.changed_at_ms = 0;
        Ok(self)
    }
}

/// Every piece in the shared loose-piece tray, in a shuffled order seeded from
/// the room identity plus `shuffle_seed` (0 for a new room; a fresh value on restart).
/// No piece starts in its own slot, so a new puzzle is never partly solved.
#[allow(clippy::too_many_arguments)]
fn scattered_pieces(
    id: &str,
    invite_code: &str,
    media_sha256: &str,
    width: u32,
    height: u32,
    rows: u16,
    columns: u16,
    shuffle_seed: u32,
) -> Vec<Piece> {
    let count = rows as usize * columns as usize;
    let tray = piece_tray_layout(width, height, rows, columns);
    let mut shuffled: Vec<usize> = (0..count).collect();
    // The server creates this arrangement once and persists it, so separate
    // rooms vary while all clients and reconnecting players see the same board.
    let mut random = 0x811c_9dc5u32;
    for byte in id
        .bytes()
        .chain(invite_code.bytes())
        .chain(media_sha256.bytes())
    {
        random = (random ^ byte as u32).wrapping_mul(0x0100_0193);
    }
    random ^= ((rows as u32) << 16) | columns as u32;
    random ^= shuffle_seed.wrapping_mul(0x9e37_79b9);
    if random == 0 {
        random = 0x9e37_79b9;
    }
    for index in (1..count).rev() {
        random ^= random << 13;
        random ^= random >> 17;
        random ^= random << 5;
        shuffled.swap(index, random as usize % (index + 1));
    }
    for index in 0..count {
        if shuffled[index] == index {
            shuffled.swap(index, (index + 1) % count);
        }
    }

    let cell_w = width as f32 / columns as f32;
    let cell_h = height as f32 / rows as f32;
    let mut pieces = Vec::with_capacity(count);
    for row in 0..rows {
        for column in 0..columns {
            let id = row as u32 * columns as u32 + column as u32;
            let (x, y) = tray_slot_position(&tray, cell_w, cell_h, shuffled[id as usize]);
            pieces.push(Piece {
                id,
                group_id: id,
                row,
                column,
                x,
                y,
                placed: false,
                held_by: None,
            });
        }
    }
    pieces
}

/// Top-left corner, in board coordinates, of the `location`th tray slot.
/// Slots fill each tray row left to right; locations past the last tray row
/// continue below it.
fn tray_slot_position(
    tray: &PieceTrayLayout,
    cell_w: f32,
    cell_h: f32,
    location: usize,
) -> (f32, f32) {
    (
        tray.tray_x + (location % tray.columns) as f32 * PIECE_TRAY_SPACING_CELLS * cell_w,
        tray.tray_y - tray.board_y
            + (location / tray.columns) as f32 * PIECE_TRAY_SPACING_CELLS * cell_h,
    )
}

/// How loose pieces are laid back into the tray.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayArrangement {
    /// Corners, then other edge pieces, then interior pieces, each group in
    /// puzzle order: the classic way to start a jigsaw.
    EdgesFirst,
    /// A fresh random order.
    Shuffle,
}

fn valid_media_fingerprint(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Samples per piece edge. A knob edge is a 20-segment spline sampled four
/// times per segment.
const EDGE_STEPS: usize = 80;

/// Left half of a knob in edge-normalised coordinates: `u` runs along the edge
/// (0..1), `v` is the offset in units of the cell's shorter side. A flat run
/// leads into a shoulder, a narrow neck at u≈0.455, and a round head that
/// overhangs the neck (u≈0.385 at its widest) before reaching the top.
const KNOB_LEFT: [[f32; 2]; 11] = [
    [0.0, 0.0],
    [0.30, 0.0],
    [0.38, 0.005],
    [0.43, 0.03],
    [0.455, 0.07],
    [0.45, 0.10],
    [0.40, 0.14],
    [0.385, 0.20],
    [0.40, 0.26],
    [0.45, 0.305],
    [0.5, 0.315],
];

/// A deterministic outline in source-image coordinates relative to the piece's
/// target cell origin. Adjacent pieces share the exact same knob; the client
/// can therefore render actual jigsaw silhouettes without adding shape data to
/// the network protocol.
pub fn piece_outline(snapshot: &RoomSnapshot, piece: &Piece) -> Vec<[f32; 2]> {
    let cell_w = snapshot.cell_width();
    let cell_h = snapshot.cell_height();
    let depth = cell_w.min(cell_h);
    let puzzle_seed = snapshot.effective_shape_seed();
    // Boundary profiles are keyed by grid edge, so both neighbours derive the
    // identical mating contour. Profiles run in the canonical direction
    // (left→right, top→bottom).
    let top = edge_profile(
        horizontal_edge_seed(piece.row, piece.column, puzzle_seed),
        piece.row > 0,
    );
    let bottom = edge_profile(
        horizontal_edge_seed(piece.row + 1, piece.column, puzzle_seed),
        piece.row + 1 < snapshot.rows,
    );
    let left = edge_profile(
        vertical_edge_seed(piece.row, piece.column, puzzle_seed),
        piece.column > 0,
    );
    let right = edge_profile(
        vertical_edge_seed(piece.row, piece.column + 1, puzzle_seed),
        piece.column + 1 < snapshot.columns,
    );
    let mut points = Vec::with_capacity(EDGE_STEPS * 4);
    // Clockwise around the silhouette.
    for [u, v] in &top {
        points.push([cell_w * u, depth * v]);
    }
    for [u, v] in right.iter().skip(1) {
        points.push([cell_w + depth * v, cell_h * u]);
    }
    for [u, v] in bottom.iter().rev().skip(1) {
        points.push([cell_w * u, cell_h + depth * v]);
    }
    // The final left sample is the first top point; skip it to avoid a
    // duplicate closing vertex.
    for [u, v] in left.iter().rev().skip(1).take(EDGE_STEPS - 1) {
        points.push([depth * v, cell_h * u]);
    }
    points
}

fn horizontal_edge_seed(boundary_row: u16, column: u16, puzzle_seed: u32) -> u32 {
    puzzle_seed ^ 0x4f1b_bcdd ^ (boundary_row as u32).wrapping_mul(0x9e37_79b9) ^ column as u32
}

fn vertical_edge_seed(row: u16, boundary_column: u16, puzzle_seed: u32) -> u32 {
    puzzle_seed ^ 0x8127_3a4d ^ (boundary_column as u32).wrapping_mul(0x85eb_ca6b) ^ row as u32
}

/// `EDGE_STEPS + 1` points `[u, v]` along one grid edge. Outer borders are
/// flat; inner edges carry a knob whose direction, size and position along the
/// edge vary with the seed.
fn edge_profile(seed: u32, internal: bool) -> Vec<[f32; 2]> {
    if !internal {
        return (0..=EDGE_STEPS)
            .map(|step| [step as f32 / EDGE_STEPS as f32, 0.0])
            .collect();
    }
    let mixed = seed.wrapping_mul(0x7feb_352d).wrapping_add(0x846c_a68b);
    let sign = if mixed & 1 == 0 { 1.0 } else { -1.0 };
    let shift = (((mixed >> 8) & 0xff) as f32 / 255.0 - 0.5) * 0.08;
    let size = 0.92 + ((mixed >> 16) & 0xff) as f32 / 255.0 * 0.16;
    // Scale the knob about the edge midpoint, mirror it, then slide it along
    // the edge; the end points stay pinned to the corners.
    let scaled: Vec<[f32; 2]> = KNOB_LEFT
        .iter()
        .enumerate()
        .map(|(index, key)| {
            if index == 0 {
                *key
            } else {
                [0.5 + (key[0] - 0.5) * size, key[1] * size * sign]
            }
        })
        .collect();
    let mut keys = scaled.clone();
    for key in scaled[..scaled.len() - 1].iter().rev() {
        keys.push([1.0 - key[0], key[1]]);
    }
    let last = keys.len() - 1;
    for key in &mut keys[1..last] {
        key[0] += shift;
    }
    let segments = keys.len() - 1;
    let per_segment = EDGE_STEPS / segments;
    let mut points = Vec::with_capacity(EDGE_STEPS + 1);
    for segment in 0..segments {
        let p0 = keys[segment.saturating_sub(1)];
        let p1 = keys[segment];
        let p2 = keys[segment + 1];
        let p3 = keys[(segment + 2).min(last)];
        for step in 0..per_segment {
            let t = step as f32 / per_segment as f32;
            points.push(catmull_rom(p0, p1, p2, p3, t));
        }
    }
    points.push(keys[last]);
    points
}

fn catmull_rom(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], t: f32) -> [f32; 2] {
    let t2 = t * t;
    let t3 = t2 * t;
    let axis = |i: usize| {
        0.5 * (2.0 * p1[i]
            + (-p0[i] + p2[i]) * t
            + (2.0 * p0[i] - 5.0 * p1[i] + 4.0 * p2[i] - p3[i]) * t2
            + (-p0[i] + 3.0 * p1[i] - 3.0 * p2[i] + p3[i]) * t3)
    };
    [axis(0), axis(1)]
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    CreateRoom {
        name: String,
        title: String,
        media_sha256: String,
        media_extension: String,
        width: u32,
        height: u32,
        rows: u16,
        columns: u16,
        #[serde(default)]
        initial_snapshot: Option<Box<RoomSnapshot>>,
        /// Length of the first video, so the server can advance the queue.
        #[serde(default)]
        duration_ms: u64,
        /// This device's private player key; see `NameClaim`.
        #[serde(default)]
        player_key: String,
    },
    JoinRoom {
        invite_code: String,
        name: String,
        media_sha256: String,
        media_extension: String,
        /// Join to watch: see everything, change nothing (protocol v13).
        #[serde(default)]
        spectator: bool,
        /// This device's private player key; empty from older clients.
        #[serde(default)]
        player_key: String,
    },
    DragStart {
        piece_id: u32,
    },
    DragStartSelection {
        piece_ids: Vec<u32>,
    },
    MovePiece {
        piece_id: u32,
        x: f32,
        y: f32,
    },
    DragEnd {
        piece_id: u32,
        x: f32,
        y: f32,
    },
    MoveSelection {
        anchor_piece_id: u32,
        x: f32,
        y: f32,
    },
    DragEndSelection {
        anchor_piece_id: u32,
        x: f32,
        y: f32,
    },
    SetPlayback {
        playing: bool,
        position_ms: u64,
        rate: f32,
        loop_start_ms: u64,
        loop_end_ms: u64,
    },
    /// Host-only: scatter all pieces back into the tray for a fresh game.
    RestartPuzzle,
    /// Any player: lay the loose single pieces back into the tray in an
    /// orderly (or shuffled) arrangement. Joined groups, placed pieces, and
    /// pieces someone is holding stay where they are.
    RearrangeTray {
        arrangement: TrayArrangement,
    },
    /// Host-only queue changes. All are refused while the video is locked
    /// (except `SetVideoLock` itself).
    QueueAdd {
        sha256: String,
        extension: String,
        title: String,
        width: u32,
        height: u32,
        #[serde(default)]
        duration_ms: u64,
    },
    QueueRemove {
        index: usize,
    },
    QueueMove {
        from: usize,
        to: usize,
    },
    /// The hot swap: make another queue item current.
    SelectItem {
        index: usize,
    },
    NextItem,
    PrevItem,
    SetRepeat {
        mode: RepeatMode,
    },
    SetVideoLock {
        locked: bool,
    },
    /// Where this player's pointer is on the table, in board coordinates
    /// (source-image pixels from the board's top-left). Relayed to the other
    /// players and never stored.
    Cursor {
        x: f32,
        y: f32,
    },
    /// The pointer left the table or the window lost focus.
    CursorHidden,
    /// A line of room chat. The server tidies it, stamps the sender and
    /// time, and sends it to everyone in the room, the sender included.
    Chat {
        text: String,
    },
    /// "Look here": a marker at a spot on the table, in board coordinates,
    /// shown to everyone for a few seconds. Relayed and never stored.
    Ping {
        x: f32,
        y: f32,
    },
    /// Make a holder: a private sorting area on the open table, in board
    /// coordinates. Pieces whose centre lies in it are reserved for its
    /// owner (protocol v12).
    CreateHolder {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        #[serde(default)]
        label: String,
    },
    /// The owner moves, resizes or renames a holder. Moving carries the
    /// loose pieces inside along.
    UpdateHolder {
        holder_id: u32,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        label: String,
    },
    /// The owner (or the host) removes a holder; its pieces stay put.
    DeleteHolder {
        holder_id: u32,
    },
    /// Ask for the current solve's timelapse; the host answers this
    /// connection alone with `ServerMessage::Timelapse` (protocol v13).
    RequestTimelapse,
    /// The host frees an absent player's name: its claim and holders go, so
    /// anyone may play under it again.
    ReleaseName {
        name: String,
    },
}

/// One line of room chat as the server relays it. Kept in the room's memory
/// for players who join later, never in the saved snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatLine {
    /// Counts up from 1 in each room while the server runs.
    pub id: u64,
    pub client_id: String,
    pub name: String,
    pub text: String,
    /// Server clock, milliseconds since the Unix epoch.
    pub sent_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome {
        client_id: String,
        name: String,
        peers: Vec<String>,
        snapshot: RoomSnapshot,
        media_upload_token: Option<String>,
        /// Whether this connection joined to watch (protocol v13).
        #[serde(default)]
        spectator: bool,
        /// Who else is watching, by name.
        #[serde(default)]
        spectators: Vec<String>,
        /// The server's wall clock when it sent this, so clients can show
        /// server-stamped times (the solve clock, chat ages) without trusting
        /// their own clock. Zero when unknown.
        #[serde(default)]
        server_now_ms: u64,
    },
    Snapshot {
        snapshot: RoomSnapshot,
    },
    PeerJoined {
        name: String,
        /// They came to watch, not to play.
        #[serde(default)]
        spectator: bool,
    },
    PeerLeft {
        name: String,
        #[serde(default)]
        spectator: bool,
    },
    PiecesChanged {
        pieces: Vec<Piece>,
        revision: u64,
        /// The room's solve stats, sent only when this change altered them:
        /// the clock started or finished, someone scored, or a restart.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stats: Option<SolveStats>,
        /// Every holder in the room, sent only when this change altered them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        holders: Option<Vec<PieceHolder>>,
    },
    PlaybackChanged {
        playback: PlaybackState,
        revision: u64,
    },
    MediaUploadToken {
        client_id: String,
        token: String,
        /// Which queue item the token is for; `None` means the room's first
        /// media (older servers).
        #[serde(default)]
        sha256: Option<String>,
    },
    Error {
        message: String,
    },
    /// Another player's pointer, in board coordinates. Never sent back to
    /// the player it belongs to.
    Cursor {
        client_id: String,
        name: String,
        x: f32,
        y: f32,
    },
    /// Another player's pointer left the table, or the player left.
    CursorHidden {
        client_id: String,
    },
    /// A chat line, sent to everyone including its author.
    Chat(ChatLine),
    /// Recent chat, oldest first, sent once right after `Welcome`.
    ChatHistory {
        lines: Vec<ChatLine>,
    },
    /// A player's marker on the table, sent to everyone including them.
    Ping {
        client_id: String,
        name: String,
        x: f32,
        y: f32,
    },
    /// The current solve's timelapse, sent only to whoever asked.
    Timelapse {
        timelapse: Timelapse,
    },
}

/// Tidy a chat line the way the server does: line breaks become spaces,
/// other control and text-direction characters go, surrounding space is trimmed, and it is cut to
/// `MAX_CHAT_CHARS`. `None` when nothing is left to say.
pub fn clean_chat_text(text: &str) -> Option<String> {
    let text: String = text
        .chars()
        // Line breaks and tabs become spaces so words stay apart.
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| {
            !c.is_control()
                && !matches!(
                    c,
                    // Direction overrides, embeddings, isolates, and marks.
                    '\u{202a}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                        | '\u{200e}'
                        | '\u{200f}'
                        | '\u{061c}'
                        // Invisible spaces, joiners, and word joiners
                        // (ZWJ stays for emoji sequences).
                        | '\u{200b}'
                        | '\u{200c}'
                        | '\u{2060}'..='\u{2064}'
                        | '\u{feff}'
                        | '\u{00ad}'
                )
        })
        .collect();
    let text: String = text.trim().chars().take(MAX_CHAT_CHARS).collect();
    let text = text.trim_end();
    (!text.is_empty()).then(|| text.to_owned())
}

#[derive(Default)]
pub struct RoomRuntime {
    pub snapshot: Option<RoomSnapshot>,
    /// Players in the room by client id. Spectators are not here.
    pub names: HashMap<String, String>,
    /// Spectators by client id: they see the room but cannot change it.
    pub spectators: HashMap<String, String>,
    /// Where pieces settled during the current solve. Host-only: saved
    /// with the room, sent on request, never in a snapshot.
    pub timelapse: Timelapse,
    pub host_id: Option<String>,
    /// Which player key owns each name in this room (see `claims`). Host
    /// state only: hosts persist it beside the snapshot, never send it.
    pub name_claims: Vec<NameClaim>,
    drag_groups: HashMap<String, Vec<u32>>,
}

impl RoomRuntime {
    pub fn forget_drag(&mut self, client_id: &str) {
        self.drag_groups.remove(client_id);
    }

    /// Acquire every linked group named by a player's selection atomically.
    /// The server records the expanded group IDs so subsequent move packets
    /// cannot add unleased pieces to the drag.
    pub fn acquire_selection(
        &mut self,
        client_id: &str,
        piece_ids: &[u32],
    ) -> Result<(Vec<Piece>, u64), String> {
        if piece_ids.is_empty() || piece_ids.len() > 400 {
            return Err("Select between 1 and 400 loose pieces".into());
        }
        if self.drag_groups.contains_key(client_id) {
            return Err("Finish the current piece drag first".into());
        }
        let snapshot = self.snapshot.as_mut().ok_or("Room is not initialized")?;
        let mut groups = HashSet::new();
        for piece_id in piece_ids {
            let piece = snapshot.piece(*piece_id).ok_or("Unknown piece")?;
            groups.insert(piece.group_id);
        }
        let members: Vec<_> = snapshot
            .pieces
            .iter()
            .filter(|piece| groups.contains(&piece.group_id))
            .collect();
        if members.iter().any(|piece| piece.placed) {
            return Err("Placed pieces cannot be selected".into());
        }
        let player = self.names.get(client_id).map(String::as_str);
        if let Some(holder) = groups
            .iter()
            .find_map(|group| snapshot.reserved_from(*group, player))
        {
            return Err(format!("That piece is in {}'s holder", holder.owner));
        }
        if members.iter().any(|piece| {
            piece
                .held_by
                .as_deref()
                .is_some_and(|owner| owner != client_id)
        }) {
            return Err("Another player is moving part of that selection".into());
        }
        for piece in &mut snapshot.pieces {
            if groups.contains(&piece.group_id) {
                piece.held_by = Some(client_id.to_owned());
            }
        }
        let mut group_ids: Vec<_> = groups.into_iter().collect();
        group_ids.sort_unstable();
        self.drag_groups
            .insert(client_id.to_owned(), group_ids.clone());
        snapshot.revision += 1;
        let pieces = snapshot
            .pieces
            .iter()
            .filter(|piece| group_ids.contains(&piece.group_id))
            .cloned()
            .collect();
        Ok((pieces, snapshot.revision))
    }

    /// Move all groups acquired by `acquire_selection` as a single rigid
    /// selection. A release resolves snapping independently for each group.
    pub fn move_selection(
        &mut self,
        client_id: &str,
        anchor_piece_id: u32,
        x: f32,
        y: f32,
        release: bool,
    ) -> Result<(Vec<Piece>, u64), String> {
        if !x.is_finite() || !y.is_finite() {
            return Err("Piece coordinates must be finite".into());
        }
        let group_ids = self
            .drag_groups
            .get(client_id)
            .cloned()
            .ok_or("You do not hold a piece selection")?;
        let (selected_ids, representatives) = {
            let snapshot = self.snapshot.as_mut().ok_or("Room is not initialized")?;
            let anchor = snapshot
                .piece(anchor_piece_id)
                .ok_or("Unknown anchor piece")?
                .clone();
            if !group_ids.contains(&anchor.group_id) || anchor.held_by.as_deref() != Some(client_id)
            {
                return Err("The anchor piece is not part of your selection".into());
            }
            let selected: Vec<_> = snapshot
                .pieces
                .iter()
                .filter(|piece| group_ids.contains(&piece.group_id))
                .collect();
            if selected.is_empty()
                || selected
                    .iter()
                    .any(|piece| piece.held_by.as_deref() != Some(client_id))
            {
                return Err("Your piece selection is no longer held".into());
            }
            let (dx, dy) = snapshot.clamp_move(
                selected.iter().map(|piece| (piece.x, piece.y)),
                x - anchor.x,
                y - anchor.y,
            );
            for piece in &mut snapshot.pieces {
                if group_ids.contains(&piece.group_id) {
                    piece.x += dx;
                    piece.y += dy;
                }
            }
            let selected_ids = snapshot
                .pieces
                .iter()
                .filter(|piece| group_ids.contains(&piece.group_id))
                .map(|piece| piece.id)
                .collect::<HashSet<_>>();
            let representatives = group_ids
                .iter()
                .filter_map(|group_id| {
                    snapshot
                        .pieces
                        .iter()
                        .find(|piece| piece.group_id == *group_id)
                        .map(|piece| piece.id)
                })
                .collect::<Vec<_>>();
            (selected_ids, representatives)
        };

        if release {
            self.drag_groups.remove(client_id);
            let mut affected_ids = selected_ids;
            for piece_id in representatives {
                let Some(piece) = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.piece(piece_id))
                    .cloned()
                else {
                    continue;
                };
                if piece.held_by.as_deref() != Some(client_id) {
                    continue;
                }
                let (changed, _) = self.move_piece(client_id, piece_id, piece.x, piece.y, true)?;
                affected_ids.extend(changed.into_iter().map(|piece| piece.id));
            }
            let snapshot = self.snapshot.as_ref().ok_or("Room is not initialized")?;
            let pieces = snapshot
                .pieces
                .iter()
                .filter(|piece| affected_ids.contains(&piece.id))
                .cloned()
                .collect();
            Ok((pieces, snapshot.revision))
        } else {
            let snapshot = self.snapshot.as_mut().ok_or("Room is not initialized")?;
            snapshot.revision += 1;
            let pieces = snapshot
                .pieces
                .iter()
                .filter(|piece| selected_ids.contains(&piece.id))
                .cloned()
                .collect();
            Ok((pieces, snapshot.revision))
        }
    }

    /// Host-only: scatter every piece back into the tray with a fresh shuffle
    /// and drop all drag leases. `shuffle_seed` varies the arrangement between restarts.
    pub fn restart(
        &mut self,
        client_id: &str,
        shuffle_seed: u32,
    ) -> Result<(Vec<Piece>, u64), String> {
        let snapshot = self.snapshot.as_mut().ok_or("Room is not ready")?;
        // `snapshot.host_id` is authoritative: it is what clients see and what
        // a promoted host (after a server restart) updates.
        if snapshot.host_id != client_id {
            return Err("Only the host can restart the puzzle".into());
        }
        snapshot.pieces = scattered_pieces(
            &snapshot.id,
            &snapshot.invite_code,
            &snapshot.media_sha256,
            snapshot.width,
            snapshot.height,
            snapshot.rows,
            snapshot.columns,
            shuffle_seed | 1,
        );
        snapshot.stats = SolveStats::default();
        snapshot.holders.clear();
        snapshot.revision += 1;
        self.drag_groups.clear();
        Ok((snapshot.pieces.clone(), snapshot.revision))
    }

    /// Lay every loose, unheld, single-piece group back into free tray slots.
    /// `shuffle_seed` varies a shuffle between calls. Slots already covered by pieces
    /// that stay put (for instance a section assembled inside the tray) are
    /// skipped, so nothing ends up stacked.
    pub fn rearrange_tray(
        &mut self,
        arrangement: TrayArrangement,
        shuffle_seed: u32,
    ) -> Result<(Vec<Piece>, u64), String> {
        let snapshot = self.snapshot.as_mut().ok_or("Room is not ready")?;
        let mut group_sizes: HashMap<u32, usize> = HashMap::new();
        for piece in &snapshot.pieces {
            *group_sizes.entry(piece.group_id).or_default() += 1;
        }
        // Pieces in anyone's holder are sorted already; leave them be.
        let is_loose = |piece: &Piece| {
            !piece.placed
                && piece.held_by.is_none()
                && group_sizes[&piece.group_id] == 1
                && snapshot.holder_of(piece).is_none()
        };
        let mut movable: Vec<u32> = snapshot
            .pieces
            .iter()
            .filter(|piece| is_loose(piece))
            .map(|piece| piece.id)
            .collect();
        if movable.is_empty() {
            return Err("There are no loose pieces to arrange".into());
        }
        let (rows, columns) = (snapshot.rows, snapshot.columns);
        match arrangement {
            TrayArrangement::EdgesFirst => movable.sort_by_key(|id| {
                let piece = snapshot.piece(*id).expect("id came from this snapshot");
                let on_row_edge = piece.row == 0 || piece.row + 1 == rows;
                let on_column_edge = piece.column == 0 || piece.column + 1 == columns;
                let kind = match (on_row_edge, on_column_edge) {
                    (true, true) => 0,
                    (false, false) => 2,
                    _ => 1,
                };
                (kind, piece.row, piece.column)
            }),
            TrayArrangement::Shuffle => {
                let mut random = 0x811c_9dc5u32 ^ shuffle_seed.wrapping_mul(0x9e37_79b9);
                for byte in snapshot.id.bytes().chain(snapshot.invite_code.bytes()) {
                    random = (random ^ u32::from(byte)).wrapping_mul(0x0100_0193);
                }
                if random == 0 {
                    random = 0x9e37_79b9;
                }
                for index in (1..movable.len()).rev() {
                    random ^= random << 13;
                    random ^= random >> 17;
                    random ^= random << 5;
                    movable.swap(index, random as usize % (index + 1));
                }
            }
        }

        let cell_w = snapshot.cell_width();
        let cell_h = snapshot.cell_height();
        let tray = piece_tray_layout(snapshot.width, snapshot.height, rows, columns);
        let scene = snapshot.scene_bounds();
        let staying: Vec<(f32, f32)> = snapshot
            .pieces
            .iter()
            .filter(|piece| !movable.contains(&piece.id))
            .map(|piece| (piece.x, piece.y))
            .collect();
        let mut slots = Vec::with_capacity(movable.len());
        let mut location = 0;
        while slots.len() < movable.len() {
            // Enough rows to hold every piece even if many slots are covered.
            if location > movable.len() * 8 + staying.len() * 4 + tray.columns {
                return Err("There is no free room in the tray".into());
            }
            let (x, y) = tray_slot_position(&tray, cell_w, cell_h, location);
            location += 1;
            let on_table = x >= scene.min_x
                && x + cell_w <= scene.max_x
                && y >= scene.min_y
                && y + cell_h <= scene.max_y;
            let covered = staying
                .iter()
                .any(|(sx, sy)| (sx - x).abs() < cell_w * 0.9 && (sy - y).abs() < cell_h * 0.9)
                || snapshot
                    .holders
                    .iter()
                    .any(|holder| holder.contains(x + cell_w * 0.5, y + cell_h * 0.5));
            if on_table && !covered {
                slots.push((x, y));
            }
        }
        for (id, (x, y)) in movable.iter().zip(slots) {
            if let Some(piece) = snapshot.piece_mut(*id) {
                piece.x = x;
                piece.y = y;
            }
        }
        snapshot.revision += 1;
        let pieces = movable
            .iter()
            .filter_map(|id| snapshot.piece(*id).cloned())
            .collect();
        Ok((pieces, snapshot.revision))
    }

    pub fn acquire(&mut self, client_id: &str, piece_id: u32) -> Result<(Vec<Piece>, u64), String> {
        let snapshot = self.snapshot.as_mut().ok_or("Room is not initialized")?;
        let group_id = snapshot.piece(piece_id).ok_or("Unknown piece")?.group_id;
        let members: Vec<_> = snapshot
            .pieces
            .iter()
            .filter(|p| p.group_id == group_id)
            .collect();
        if members.iter().any(|piece| piece.placed) {
            return Err("That piece group is already placed".into());
        }
        let player = self.names.get(client_id).map(String::as_str);
        if let Some(holder) = snapshot.reserved_from(group_id, player) {
            return Err(format!("That piece is in {}'s holder", holder.owner));
        }
        if members.iter().any(|piece| {
            piece
                .held_by
                .as_deref()
                .is_some_and(|owner| owner != client_id)
        }) {
            return Err("Another player is moving that piece group".into());
        }
        for piece in &mut snapshot.pieces {
            if piece.group_id == group_id {
                piece.held_by = Some(client_id.to_owned());
            }
        }
        snapshot.revision += 1;
        let members = snapshot
            .pieces
            .iter()
            .filter(|p| p.group_id == group_id)
            .cloned()
            .collect();
        Ok((members, snapshot.revision))
    }

    pub fn move_piece(
        &mut self,
        client_id: &str,
        piece_id: u32,
        x: f32,
        y: f32,
        release: bool,
    ) -> Result<(Vec<Piece>, u64), String> {
        if !x.is_finite() || !y.is_finite() {
            return Err("Piece coordinates must be finite".into());
        }
        let snapshot = self.snapshot.as_mut().ok_or("Room is not initialized")?;
        let cell_w = snapshot.cell_width();
        let cell_h = snapshot.cell_height();
        let active = snapshot.piece(piece_id).ok_or("Unknown piece")?.clone();
        if active.placed {
            return Err("That piece is already placed".into());
        }
        if active.held_by.as_deref() != Some(client_id) {
            return Err("You do not hold that piece group".into());
        }
        let group_id = active.group_id;
        let (dx, dy) = snapshot.clamp_move(
            snapshot
                .pieces
                .iter()
                .filter(|p| p.group_id == group_id)
                .map(|p| (p.x, p.y)),
            x - active.x,
            y - active.y,
        );
        for piece in &mut snapshot.pieces {
            if piece.group_id == group_id {
                piece.x += dx;
                piece.y += dy;
            }
        }

        if release {
            let moved = snapshot.piece(piece_id).unwrap().clone();
            let target_x = moved.column as f32 * cell_w;
            let target_y = moved.row as f32 * cell_h;
            let tolerance = cell_w.min(cell_h) * 0.34;
            if (moved.x - target_x).hypot(moved.y - target_y) <= tolerance {
                let offset_x = target_x - moved.x;
                let offset_y = target_y - moved.y;
                for piece in &mut snapshot.pieces {
                    if piece.group_id == group_id {
                        piece.x += offset_x;
                        piece.y += offset_y;
                        piece.placed = true;
                        piece.held_by = None;
                    }
                }
            } else if let Some((other_group, offset_x, offset_y, placed)) =
                find_neighbor_merge(snapshot, group_id, cell_w, cell_h, tolerance)
            {
                for piece in &mut snapshot.pieces {
                    if piece.group_id == group_id {
                        piece.x += offset_x;
                        piece.y += offset_y;
                        piece.group_id = other_group;
                    }
                }
                for piece in &mut snapshot.pieces {
                    if piece.group_id == other_group {
                        piece.held_by = None;
                        if placed {
                            piece.placed = true;
                        }
                    }
                }
            } else {
                for piece in &mut snapshot.pieces {
                    if piece.group_id == group_id {
                        piece.held_by = None;
                    }
                }
            }
        }
        snapshot.revision += 1;
        let resulting_group = snapshot.piece(piece_id).unwrap().group_id;
        let members = snapshot
            .pieces
            .iter()
            .filter(|p| p.group_id == resulting_group)
            .cloned()
            .collect();
        Ok((members, snapshot.revision))
    }

    // Mirror the room protocol's playback command without a second DTO layer.
    #[allow(clippy::too_many_arguments)]
    pub fn set_playback(
        &mut self,
        client_id: &str,
        playing: bool,
        position_ms: u64,
        rate: f32,
        loop_start_ms: u64,
        loop_end_ms: u64,
        now_ms: u64,
    ) -> Result<(PlaybackState, u64), String> {
        let snapshot = self.snapshot.as_mut().ok_or("Room is not initialized")?;
        if snapshot.host_id != client_id {
            return Err("Only the room host can control shared playback".into());
        }
        if !rate.is_finite() || !(0.25..=2.0).contains(&rate) {
            return Err("Playback speed must be between 0.25× and 2×".into());
        }
        if loop_end_ms > 0 && loop_start_ms >= loop_end_ms {
            return Err("Loop end must be after loop start".into());
        }
        // A loop that starts at or after the end of the video would restart
        // already finished, and a host would tick the room forever.
        let end = if loop_end_ms > 0 {
            loop_end_ms
        } else {
            snapshot.current_item().map_or(0, |item| item.duration_ms)
        };
        if end > 0 && loop_start_ms >= end {
            return Err("Loop start must be before the end of the video".into());
        }
        snapshot.playback = PlaybackState {
            playing,
            position_ms,
            rate,
            loop_start_ms,
            loop_end_ms,
            changed_at_ms: now_ms,
        };
        snapshot.revision += 1;
        Ok((snapshot.playback.clone(), snapshot.revision))
    }

    fn queue_guard(&mut self, client_id: &str) -> Result<&mut RoomSnapshot, String> {
        let snapshot = self.snapshot.as_mut().ok_or("Room is not initialized")?;
        if snapshot.host_id != client_id {
            return Err("Only the room host can change the video queue".into());
        }
        if snapshot.video_lock {
            return Err("The video is locked. Unlock it to change the queue.".into());
        }
        Ok(snapshot)
    }

    /// Make `index` current, remembering where the old item was and resuming
    /// the new item where it was left. Pieces are untouched.
    fn switch_item(snapshot: &mut RoomSnapshot, index: usize, now_ms: u64) {
        let position = snapshot.playback.position_at(now_ms);
        if let Some(old) = snapshot.queue.get_mut(snapshot.current) {
            old.position_ms = position;
            old.loop_start_ms = snapshot.playback.loop_start_ms;
            old.loop_end_ms = snapshot.playback.loop_end_ms;
        }
        snapshot.current = index;
        let item = &snapshot.queue[index];
        snapshot.playback = PlaybackState {
            // A video nobody can load yet must not run the room clock.
            playing: snapshot.playback.playing && item.available,
            position_ms: item.position_ms,
            rate: snapshot.playback.rate,
            loop_start_ms: item.loop_start_ms,
            loop_end_ms: item.loop_end_ms,
            changed_at_ms: now_ms,
        };
        snapshot.sync_current_mirror();
    }

    pub fn queue_add(
        &mut self,
        client_id: &str,
        mut item: MediaItem,
    ) -> Result<RoomSnapshot, String> {
        let snapshot = self.queue_guard(client_id)?;
        if !valid_media_fingerprint(&item.sha256) {
            return Err("That media has an invalid fingerprint".into());
        }
        if valid_media_extension(&item.extension).is_none() {
            return Err("That media type is not supported".into());
        }
        if !valid_media_dimensions(item.width, item.height) {
            return Err("That media is larger than this room allows".into());
        }
        if snapshot.queue.len() >= MAX_QUEUE_ITEMS {
            return Err(format!(
                "The queue can hold at most {MAX_QUEUE_ITEMS} videos"
            ));
        }
        if snapshot
            .queue
            .iter()
            .any(|existing| existing.sha256 == item.sha256)
        {
            return Err("That video is already in the queue".into());
        }
        item.available = false;
        item.position_ms = 0;
        item.loop_start_ms = 0;
        item.loop_end_ms = 0;
        snapshot.queue.push(item);
        snapshot.revision += 1;
        Ok(snapshot.clone())
    }

    pub fn queue_remove(
        &mut self,
        client_id: &str,
        index: usize,
        now_ms: u64,
    ) -> Result<RoomSnapshot, String> {
        let snapshot = self.queue_guard(client_id)?;
        if index >= snapshot.queue.len() {
            return Err("No such queue item".into());
        }
        if snapshot.queue.len() == 1 {
            return Err("A room needs at least one video".into());
        }
        if index == snapshot.current {
            let next = if index + 1 < snapshot.queue.len() {
                index + 1
            } else {
                index - 1
            };
            Self::switch_item(snapshot, next, now_ms);
            snapshot.queue.remove(index);
            if next > index {
                snapshot.current = next - 1;
            }
        } else {
            snapshot.queue.remove(index);
            if index < snapshot.current {
                snapshot.current -= 1;
            }
        }
        snapshot.sync_current_mirror();
        snapshot.revision += 1;
        Ok(snapshot.clone())
    }

    pub fn queue_move(
        &mut self,
        client_id: &str,
        from: usize,
        to: usize,
    ) -> Result<RoomSnapshot, String> {
        let snapshot = self.queue_guard(client_id)?;
        let len = snapshot.queue.len();
        if from >= len || to >= len {
            return Err("No such queue item".into());
        }
        let current_sha = snapshot.media_sha256.clone();
        let item = snapshot.queue.remove(from);
        snapshot.queue.insert(to, item);
        snapshot.current = snapshot
            .queue
            .iter()
            .position(|item| item.sha256 == current_sha)
            .unwrap_or(0);
        snapshot.sync_current_mirror();
        snapshot.revision += 1;
        Ok(snapshot.clone())
    }

    pub fn select_item(
        &mut self,
        client_id: &str,
        index: usize,
        now_ms: u64,
    ) -> Result<RoomSnapshot, String> {
        let snapshot = self.queue_guard(client_id)?;
        if index >= snapshot.queue.len() {
            return Err("No such queue item".into());
        }
        if index != snapshot.current {
            Self::switch_item(snapshot, index, now_ms);
            snapshot.revision += 1;
        }
        Ok(snapshot.clone())
    }

    pub fn step_item(
        &mut self,
        client_id: &str,
        forward: bool,
        now_ms: u64,
    ) -> Result<RoomSnapshot, String> {
        let snapshot = self.queue_guard(client_id)?;
        let len = snapshot.queue.len();
        if len == 0 {
            return Err("The queue is empty".into());
        }
        let index = if forward {
            (snapshot.current + 1) % len
        } else {
            (snapshot.current + len - 1) % len
        };
        if index != snapshot.current {
            Self::switch_item(snapshot, index, now_ms);
            snapshot.revision += 1;
        }
        Ok(snapshot.clone())
    }

    pub fn set_repeat(
        &mut self,
        client_id: &str,
        mode: RepeatMode,
    ) -> Result<RoomSnapshot, String> {
        let snapshot = self.queue_guard(client_id)?;
        snapshot.repeat = mode;
        snapshot.revision += 1;
        Ok(snapshot.clone())
    }

    pub fn set_video_lock(
        &mut self,
        client_id: &str,
        locked: bool,
    ) -> Result<RoomSnapshot, String> {
        let snapshot = self.snapshot.as_mut().ok_or("Room is not initialized")?;
        if snapshot.host_id != client_id {
            return Err("Only the room host can lock the video".into());
        }
        snapshot.video_lock = locked;
        snapshot.revision += 1;
        Ok(snapshot.clone())
    }

    /// When `tick_advance` will next have something to do, in the same clock
    /// as `now_ms`: `now_ms` if the item is already past its end, `None` if
    /// nothing will advance by itself (paused, repeat-one, no known length).
    /// Hosts arm a single timer for this instead of polling.
    pub fn next_tick_deadline(&self, now_ms: u64) -> Option<u64> {
        let snapshot = self.snapshot.as_ref()?;
        if !snapshot.playback.playing
            || snapshot.repeat == RepeatMode::One
            || snapshot.queue.is_empty()
        {
            return None;
        }
        let end = if snapshot.playback.loop_end_ms > 0 {
            snapshot.playback.loop_end_ms
        } else {
            snapshot.current_item()?.duration_ms
        };
        if end == 0 {
            return None;
        }
        let position = snapshot.playback.position_at(now_ms);
        if position >= end {
            return Some(now_ms);
        }
        let rate = f64::from(snapshot.playback.rate);
        if !rate.is_finite() || rate <= 0.0 {
            return None;
        }
        let remaining = ((end - position) as f64 / rate).ceil();
        Some(now_ms.saturating_add(remaining as u64))
    }

    /// Advance the queue when the current item has played to its end.
    /// Called on a timer; returns the new snapshot when something changed.
    pub fn tick_advance(&mut self, now_ms: u64) -> Option<RoomSnapshot> {
        let snapshot = self.snapshot.as_mut()?;
        if !snapshot.playback.playing
            || snapshot.repeat == RepeatMode::One
            || snapshot.queue.is_empty()
        {
            return None;
        }
        let end = if snapshot.playback.loop_end_ms > 0 {
            snapshot.playback.loop_end_ms
        } else {
            snapshot.current_item()?.duration_ms
        };
        if end == 0 || snapshot.playback.position_at(now_ms) < end {
            return None;
        }
        let last = snapshot.current + 1 >= snapshot.queue.len();
        if last && snapshot.repeat == RepeatMode::Off {
            snapshot.playback.playing = false;
            snapshot.playback.position_ms = end;
            snapshot.playback.changed_at_ms = now_ms;
        } else {
            let finished = snapshot.current;
            let next = if last { 0 } else { snapshot.current + 1 };
            if next == finished {
                // One video, repeat All: just play it again from its loop in.
                // A stored loop-in at or past the end restarts from the top
                // instead, so the room cannot finish again immediately.
                snapshot.playback.position_ms = if snapshot.playback.loop_start_ms < end {
                    snapshot.playback.loop_start_ms
                } else {
                    0
                };
                snapshot.playback.changed_at_ms = now_ms;
            } else {
                Self::switch_item(snapshot, next, now_ms);
                // The finished item starts from the top next time round.
                snapshot.queue[finished].position_ms = 0;
            }
        }
        snapshot.revision += 1;
        Some(snapshot.clone())
    }
}

/// Returns the smallest translation that joins an adjacent source-image piece
/// to the moving cluster. Pieces stay axis-aligned in the first playable build.
fn find_neighbor_merge(
    snapshot: &RoomSnapshot,
    moving_group: u32,
    cell_w: f32,
    cell_h: f32,
    tolerance: f32,
) -> Option<(u32, f32, f32, bool)> {
    let mut best: Option<(u32, f32, f32, bool, f32)> = None;
    for moving in snapshot
        .pieces
        .iter()
        .filter(|piece| piece.group_id == moving_group)
    {
        for fixed in snapshot
            .pieces
            .iter()
            .filter(|piece| piece.group_id != moving_group)
        {
            if fixed.held_by.is_some() {
                continue;
            }
            let expected = if moving.row == fixed.row && moving.column + 1 == fixed.column {
                Some((fixed.x - cell_w - moving.x, fixed.y - moving.y))
            } else if moving.row == fixed.row && fixed.column + 1 == moving.column {
                Some((fixed.x + cell_w - moving.x, fixed.y - moving.y))
            } else if moving.column == fixed.column && moving.row + 1 == fixed.row {
                Some((fixed.x - moving.x, fixed.y - cell_h - moving.y))
            } else if moving.column == fixed.column && fixed.row + 1 == moving.row {
                Some((fixed.x - moving.x, fixed.y + cell_h - moving.y))
            } else {
                None
            };
            let Some((dx, dy)) = expected else {
                continue;
            };
            let distance = dx.hypot(dy);
            if distance > tolerance {
                continue;
            }
            let is_placed = fixed.placed;
            if best.is_none_or(|candidate| distance < candidate.4) {
                best = Some((fixed.group_id, dx, dy, is_placed, distance));
            }
        }
    }
    best.map(|(group, dx, dy, placed, _)| (group, dx, dy, placed))
}

#[cfg(test)]
mod tests {
    #[test]
    fn welcome_server_clock_defaults_to_unknown() {
        let snapshot = RoomSnapshot::create(
            "room-id".into(),
            "WEL123".into(),
            "Welcome".into(),
            "a".repeat(64),
            "png".into(),
            64,
            48,
            2,
            2,
            "host".into(),
        )
        .unwrap();
        let welcome = ServerMessage::Welcome {
            client_id: "c".into(),
            name: "n".into(),
            peers: Vec::new(),
            snapshot,
            media_upload_token: None,
            spectator: false,
            spectators: Vec::new(),
            server_now_ms: 42,
        };
        // An older server sends no server_now_ms; it parses as unknown (0).
        let text = serde_json::to_string(&welcome)
            .unwrap()
            .replace(",\"server_now_ms\":42", "");
        assert!(!text.contains("server_now_ms"));
        match serde_json::from_str::<ServerMessage>(&text).unwrap() {
            ServerMessage::Welcome { server_now_ms, .. } => assert_eq!(server_now_ms, 0),
            other => panic!("unexpected {other:?}"),
        }
    }

    use super::*;

    fn queue_room() -> RoomRuntime {
        let mut snapshot = RoomSnapshot::create(
            "r".into(),
            "ABCD".into(),
            "A".into(),
            "a1".repeat(32),
            "mp4".into(),
            64,
            48,
            2,
            2,
            "host".into(),
        )
        .unwrap();
        snapshot.queue[0].available = true;
        snapshot.queue[0].duration_ms = 10_000;
        snapshot.sync_current_mirror();
        RoomRuntime {
            snapshot: Some(snapshot),
            ..RoomRuntime::default()
        }
    }

    fn item(tag: &str, duration_ms: u64) -> MediaItem {
        MediaItem {
            sha256: tag.repeat(32),
            extension: "mp4".into(),
            title: tag.into(),
            width: 32,
            height: 32,
            duration_ms,
            available: true,
            position_ms: 0,
            loop_start_ms: 0,
            loop_end_ms: 0,
        }
    }

    /// Uploaded and ready to play.
    fn make_available(room: &mut RoomRuntime, index: usize) {
        let snapshot = room.snapshot.as_mut().unwrap();
        snapshot.queue[index].available = true;
        snapshot.sync_current_mirror();
    }

    #[test]
    fn single_item_queue_with_repeat_all_restarts_once_and_does_not_storm() {
        let mut room = queue_room();
        room.set_repeat("host", RepeatMode::All).unwrap();
        room.set_playback("host", true, 0, 1.0, 2_000, 0, 1_000)
            .unwrap();
        let before = room.snapshot.as_ref().unwrap().revision;
        assert!(room.tick_advance(10_999).is_none());
        let restarted = room.tick_advance(11_000).unwrap();
        assert_eq!(restarted.current, 0);
        assert!(restarted.playback.playing);
        assert_eq!(restarted.playback.position_ms, 2_000, "back to the loop in");
        assert_eq!(restarted.revision, before + 1);
        // The next ticks must find nothing to do.
        assert!(room.tick_advance(11_250).is_none());
        assert!(room.tick_advance(11_500).is_none());
        assert_eq!(room.snapshot.as_ref().unwrap().revision, before + 1);
    }

    #[test]
    fn single_item_queue_with_repeat_off_pauses_at_the_end() {
        let mut room = queue_room();
        room.set_repeat("host", RepeatMode::Off).unwrap();
        room.set_playback("host", true, 0, 1.0, 0, 0, 0).unwrap();
        let ended = room.tick_advance(10_000).unwrap();
        assert!(!ended.playback.playing);
        assert!(room.tick_advance(10_250).is_none());
    }

    #[test]
    fn switching_to_an_item_that_is_not_uploaded_yet_pauses_playback() {
        let mut room = queue_room();
        room.queue_add("host", item("b2", 5000)).unwrap();
        room.set_playback("host", true, 9_000, 1.0, 0, 0, 0)
            .unwrap();
        let swapped = room.select_item("host", 1, 100).unwrap();
        assert!(!swapped.queue[1].available);
        assert!(!swapped.playback.playing);

        // Auto-advance into an unavailable item pauses too, instead of
        // running the room clock while nobody can see the video.
        let mut room = queue_room();
        room.queue_add("host", item("b2", 5000)).unwrap();
        room.set_repeat("host", RepeatMode::All).unwrap();
        room.set_playback("host", true, 0, 1.0, 0, 0, 0).unwrap();
        let next = room.tick_advance(10_000).unwrap();
        assert_eq!(next.current, 1);
        assert!(!next.playback.playing);
    }

    #[test]
    fn only_the_host_changes_the_queue_and_lock_blocks_it() {
        let mut room = queue_room();
        assert!(room.queue_add("guest", item("b2", 5000)).is_err());
        room.queue_add("host", item("b2", 5000)).unwrap();
        room.set_video_lock("host", true).unwrap();
        assert!(room.queue_add("host", item("c3", 5000)).is_err());
        assert!(room.select_item("host", 1, 0).is_err());
        assert!(room.set_repeat("host", RepeatMode::All).is_err());
        assert!(room.set_video_lock("guest", false).is_err());
        room.set_video_lock("host", false).unwrap();
        room.select_item("host", 1, 0).unwrap();
    }

    #[test]
    fn duplicate_and_invalid_items_are_rejected() {
        let mut room = queue_room();
        assert!(room.queue_add("host", item("a1", 5000)).is_err());
        let mut bad = item("b2", 5000);
        bad.sha256 = "nothex".into();
        assert!(room.queue_add("host", bad).is_err());
        for n in 0..MAX_QUEUE_ITEMS - 1 {
            room.queue_add("host", item(&format!("{n:02x}"), 1000))
                .unwrap();
        }
        assert!(room.queue_add("host", item("ff", 1000)).is_err());
    }

    #[test]
    fn hot_swap_keeps_pieces_and_restores_each_items_position() {
        let mut room = queue_room();
        room.queue_add("host", item("b2", 5000)).unwrap();
        make_available(&mut room, 1);
        let pieces_before = room.snapshot.as_ref().unwrap().pieces.clone();
        room.set_playback("host", true, 3000, 1.0, 1000, 8000, 100)
            .unwrap();
        let swapped = room.select_item("host", 1, 600).unwrap();
        assert_eq!(swapped.current, 1);
        assert_eq!(swapped.media_sha256, "b2".repeat(32));
        assert_eq!(swapped.pieces, pieces_before);
        assert!(swapped.playback.playing);
        assert_eq!(swapped.playback.position_ms, 0);
        assert_eq!(swapped.queue[0].position_ms, 3500);
        assert_eq!(swapped.queue[0].loop_start_ms, 1000);
        let back = room.select_item("host", 0, 700).unwrap();
        assert_eq!(back.playback.position_ms, 3500);
        assert_eq!(back.playback.loop_end_ms, 8000);
    }

    #[test]
    fn removing_before_current_shifts_current_and_last_item_is_kept() {
        let mut room = queue_room();
        room.queue_add("host", item("b2", 5000)).unwrap();
        room.queue_add("host", item("c3", 5000)).unwrap();
        room.select_item("host", 2, 0).unwrap();
        let after = room.queue_remove("host", 0, 10).unwrap();
        assert_eq!(after.current, 1);
        assert_eq!(after.media_sha256, "c3".repeat(32));
        room.queue_remove("host", 1, 20).unwrap();
        let left = room.snapshot.as_ref().unwrap();
        assert_eq!(left.queue.len(), 1);
        assert!(room.queue_remove("host", 0, 30).is_err());
    }

    #[test]
    fn queue_move_tracks_the_current_item() {
        let mut room = queue_room();
        room.queue_add("host", item("b2", 5000)).unwrap();
        room.queue_add("host", item("c3", 5000)).unwrap();
        let moved = room.queue_move("host", 0, 2).unwrap();
        assert_eq!(moved.queue[2].sha256, "a1".repeat(32));
        assert_eq!(moved.current, 2);
        assert_eq!(moved.media_sha256, "a1".repeat(32));
    }

    #[test]
    fn auto_advance_follows_repeat_mode_rate_and_loop_end() {
        let mut room = queue_room();
        room.queue_add("host", item("b2", 4000)).unwrap();
        make_available(&mut room, 1);
        room.set_repeat("host", RepeatMode::All).unwrap();
        room.set_playback("host", true, 0, 2.0, 0, 0, 1_000)
            .unwrap();
        assert!(room.tick_advance(5_900).is_none());
        let next = room.tick_advance(6_000).unwrap();
        assert_eq!(next.current, 1);
        assert_eq!(next.playback.position_ms, 0);
        assert!(next.playback.playing);
        room.set_playback("host", true, 0, 1.0, 500, 2_000, 10_000)
            .unwrap();
        assert!(room.tick_advance(11_999).is_none());
        let wrapped = room.tick_advance(12_000).unwrap();
        assert_eq!(wrapped.current, 0);

        room.set_repeat("host", RepeatMode::Off).unwrap();
        room.select_item("host", 1, 20_000).unwrap();
        room.set_playback("host", true, 0, 1.0, 0, 0, 20_000)
            .unwrap();
        let ended = room.tick_advance(24_000).unwrap();
        assert_eq!(ended.current, 1);
        assert!(!ended.playback.playing);
        assert!(room.tick_advance(25_000).is_none());

        room.set_repeat("host", RepeatMode::One).unwrap();
        room.set_playback("host", true, 0, 1.0, 0, 0, 30_000)
            .unwrap();
        assert!(room.tick_advance(99_000).is_none());
    }

    #[test]
    fn items_without_a_known_end_never_auto_advance() {
        let mut room = queue_room();
        room.snapshot.as_mut().unwrap().queue[0].duration_ms = 0;
        room.queue_add("host", item("b2", 4000)).unwrap();
        room.set_repeat("host", RepeatMode::All).unwrap();
        room.set_playback("host", true, 0, 1.0, 0, 0, 0).unwrap();
        assert!(room.tick_advance(1_000_000).is_none());
    }

    #[test]
    fn v7_snapshot_migrates_to_one_item_queue_with_unchanged_contours() {
        let sha = "ab".repeat(32);
        let json = serde_json::json!({
            "protocol_version": 7, "id": "r", "invite_code": "ABCD", "title": "Clip",
            "media_sha256": sha, "media_extension": "mp4", "media_available": true,
            "width": 64, "height": 48, "rows": 2, "columns": 2,
            "pieces": (0..4).map(|i| serde_json::json!({
                "id": i, "group_id": i, "row": i / 2, "column": i % 2,
                "x": 0.0, "y": 0.0, "placed": false, "held_by": null
            })).collect::<Vec<_>>(),
            "playback": {"playing": true, "position_ms": 1500, "rate": 1.0,
                         "loop_start_ms": 1000, "loop_end_ms": 4000, "changed_at_ms": 9},
            "host_id": "h", "revision": 3
        });
        let mut snapshot: RoomSnapshot = serde_json::from_value(json).unwrap();
        let before = piece_outline(&snapshot, &snapshot.pieces[0]);
        snapshot.migrate();
        assert_eq!(snapshot.queue.len(), 1);
        assert_eq!(snapshot.current, 0);
        assert_eq!(snapshot.queue[0].sha256, sha);
        assert!(snapshot.queue[0].available);
        assert_eq!(snapshot.queue[0].loop_end_ms, 4000);
        assert_eq!(snapshot.repeat, RepeatMode::One);
        assert!(!snapshot.video_lock);
        assert_eq!(snapshot.shape_seed, fnv1a(&sha));
        assert!(snapshot.playback.playing);
        assert_eq!(piece_outline(&snapshot, &snapshot.pieces[0]), before);
    }

    #[test]
    fn contours_follow_shape_seed_not_the_current_media() {
        let mut room = RoomSnapshot::create(
            "r".into(),
            "ABCD".into(),
            "T".into(),
            "cd".repeat(32),
            "mp4".into(),
            64,
            48,
            2,
            2,
            "h".into(),
        )
        .unwrap();
        let before = piece_outline(&room, &room.pieces[0]);
        room.queue.push(MediaItem {
            sha256: "ef".repeat(32),
            extension: "mp4".into(),
            title: "B".into(),
            width: 10,
            height: 10,
            duration_ms: 1000,
            available: true,
            position_ms: 0,
            loop_start_ms: 0,
            loop_end_ms: 0,
        });
        room.current = 1;
        room.sync_current_mirror();
        assert_eq!(room.media_sha256, "ef".repeat(32));
        assert_eq!(piece_outline(&room, &room.pieces[0]), before);
    }

    #[test]
    fn create_builds_a_one_item_queue_that_repeats() {
        let room = RoomSnapshot::create(
            "r".into(),
            "ABCD".into(),
            "Show".into(),
            "cd".repeat(32),
            "mp4".into(),
            64,
            48,
            2,
            2,
            "h".into(),
        )
        .unwrap();
        assert_eq!(room.protocol_version, PROTOCOL_VERSION);
        assert_eq!(room.queue.len(), 1);
        assert_eq!(room.queue[0].title, "Show");
        assert_eq!(room.repeat, RepeatMode::One);
        assert!(!room.video_lock);
    }

    fn room() -> RoomRuntime {
        let snapshot = RoomSnapshot::create(
            "id".into(),
            "invite".into(),
            "test".into(),
            "hash".into(),
            "png".into(),
            800,
            600,
            3,
            4,
            "host".into(),
        )
        .unwrap();
        RoomRuntime {
            snapshot: Some(snapshot),
            names: HashMap::from([("host".into(), "Host".into())]),
            host_id: Some("host".into()),
            ..RoomRuntime::default()
        }
    }

    #[test]
    fn create_room_makes_bounded_grid() {
        let room = room();
        let snapshot = room.snapshot.unwrap();
        assert_eq!(snapshot.pieces.len(), 12);
        assert!(!snapshot.completed());
        assert!(
            snapshot
                .pieces
                .iter()
                .all(|p| p.x.is_finite() && p.y.is_finite())
        );
    }

    #[test]
    fn media_dimensions_have_side_and_total_pixel_limits() {
        assert!(valid_media_dimensions(8192, 4882));
        assert!(!valid_media_dimensions(0, 1));
        assert!(!valid_media_dimensions(8193, 1));
        assert!(!valid_media_dimensions(8192, 8192));
        assert!(!valid_media_dimensions(u32::MAX, u32::MAX));
        let oversized = RoomSnapshot::create(
            "id".into(),
            "invite".into(),
            "oversized".into(),
            "hash".into(),
            "png".into(),
            8192,
            8192,
            2,
            2,
            "host".into(),
        );
        assert!(oversized.unwrap_err().contains("pixels total"));
    }

    #[test]
    fn rehosting_a_snapshot_preserves_progress_but_resets_room_identity() {
        let mut saved = RoomSnapshot::create(
            "old-room".into(),
            "OLD123".into(),
            "Loop puzzle".into(),
            "a".repeat(64),
            "mp4".into(),
            640,
            360,
            2,
            2,
            "old-host".into(),
        )
        .unwrap();
        saved.pieces[0].placed = true;
        saved.pieces[0].held_by = Some("stale-player".into());
        saved.playback.playing = true;
        saved.playback.position_ms = 1200;
        saved.media_available = true;
        saved.revision = 77;

        let room = saved
            .rehost("new-room".into(), "NEW123".into(), "new-host".into())
            .unwrap();

        assert_eq!(room.id, "new-room");
        assert_eq!(room.invite_code, "NEW123");
        assert_eq!(room.host_id, "new-host");
        assert_eq!(room.protocol_version, PROTOCOL_VERSION);
        assert_eq!(room.revision, 1);
        assert!(!room.media_available);
        assert!(room.pieces[0].placed);
        assert_eq!(room.pieces[0].held_by, None);
        assert!(!room.playback.playing);
        assert_eq!(room.playback.position_ms, 1200);
    }

    #[test]
    fn rehosting_rejects_malformed_piece_state() {
        let mut saved = RoomSnapshot::create(
            "old-room".into(),
            "OLD123".into(),
            "Puzzle".into(),
            "b".repeat(64),
            "png".into(),
            32,
            24,
            2,
            2,
            "old-host".into(),
        )
        .unwrap();
        saved.pieces[0].x = f32::NAN;

        assert!(
            saved
                .rehost("new-room".into(), "NEW123".into(), "new-host".into())
                .is_err()
        );
    }

    #[test]
    fn new_room_starts_loose_pieces_in_a_distinct_tray_beside_the_board() {
        let snapshot = room().snapshot.unwrap();
        let tray = piece_tray_layout(
            snapshot.width,
            snapshot.height,
            snapshot.rows,
            snapshot.columns,
        );
        assert!(tray.tray_x > snapshot.width as f32);
        assert!(tray.content_width > tray.tray_x + tray.tray_width);
        assert!(tray.content_height >= snapshot.height as f32);
        assert!(
            snapshot
                .pieces
                .iter()
                .all(|piece| !piece.placed && piece.x >= tray.tray_x)
        );
        let unique_slots = snapshot
            .pieces
            .iter()
            .map(|piece| (piece.x.to_bits(), piece.y.to_bits()))
            .collect::<HashSet<_>>();
        assert_eq!(unique_slots.len(), snapshot.pieces.len());
    }

    #[test]
    fn room_layout_varies_by_identity_and_never_starts_with_correctly_placed_pieces() {
        let create = |id: &str| {
            RoomSnapshot::create(
                id.into(),
                format!("CODE{id}"),
                "Shuffle test".into(),
                "same-media".into(),
                "png".into(),
                800,
                600,
                3,
                4,
                "host".into(),
            )
            .unwrap()
        };
        let first = create("room-one");
        let second = create("room-two");
        let repeated = create("room-one");
        let layout = |snapshot: &RoomSnapshot| {
            snapshot
                .pieces
                .iter()
                .map(|piece| (piece.x, piece.y))
                .collect::<Vec<_>>()
        };
        assert_ne!(layout(&first), layout(&second));
        assert_eq!(layout(&first), layout(&repeated));
        for rows in 2..=20 {
            for columns in 2..=20 {
                if rows * columns > 400 {
                    continue;
                }
                let snapshot = RoomSnapshot::create(
                    format!("grid-{rows}-{columns}"),
                    "GRID".into(),
                    "Shuffle test".into(),
                    "same-media".into(),
                    "png".into(),
                    800,
                    600,
                    rows,
                    columns,
                    "host".into(),
                )
                .unwrap();
                let tray = piece_tray_layout(800, 600, rows, columns);
                assert!(
                    snapshot.pieces.iter().all(|piece| {
                        !piece.placed
                            && piece.x + 0.001 >= tray.tray_x
                            && piece.x < tray.tray_x + tray.tray_width + 0.001
                            && piece.y + tray.board_y + 0.001 >= tray.tray_y
                            && piece.y + tray.board_y < tray.tray_y + tray.tray_height + 0.001
                            && (piece.x != piece.column as f32 * snapshot.cell_width()
                                || piece.y != piece.row as f32 * snapshot.cell_height())
                    }),
                    "grid {rows}x{columns} begins with a solved piece"
                );
            }
        }
    }

    #[test]
    fn older_room_snapshots_deserialize_with_local_media_defaults() {
        let mut stored = serde_json::to_value(room().snapshot.unwrap()).unwrap();
        let fields = stored.as_object_mut().unwrap();
        fields.insert("protocol_version".into(), serde_json::json!(2));
        fields.remove("media_extension");
        fields.remove("media_available");
        let restored: RoomSnapshot = serde_json::from_value(stored).unwrap();
        assert_eq!(restored.protocol_version, 2);
        assert_eq!(restored.media_extension, "bin");
        assert!(!restored.media_available);
    }

    #[test]
    fn piece_outlines_have_flat_outer_borders_and_shared_mating_edges() {
        let snapshot = room().snapshot.unwrap();
        let upper = snapshot.piece(0).unwrap();
        let lower = snapshot.piece(4).unwrap();
        let upper_outline = piece_outline(&snapshot, upper);
        let lower_outline = piece_outline(&snapshot, lower);
        let cell_h = snapshot.cell_height();
        let n = EDGE_STEPS;

        // Bottom runs right to left, top left to right.
        for sample in 0..=n {
            let bottom = upper_outline[2 * n + sample];
            let top = lower_outline[n - sample];
            assert!((bottom[0] - top[0]).abs() < 0.001);
            assert!((bottom[1] - cell_h - top[1]).abs() < 0.001);
        }

        let corner = snapshot.piece(0).unwrap();
        let outline = piece_outline(&snapshot, corner);
        assert!(outline[..=n].iter().all(|point| point[1].abs() < 0.001));
        assert!(
            outline[3 * n + 1..]
                .iter()
                .all(|point| point[0].abs() < 0.001)
        );
    }

    #[test]
    fn inner_edges_are_knobs_with_a_neck_narrower_than_the_head() {
        for seed in [1_u32, 99, 12345, 0xdead_beef] {
            let profile = edge_profile(seed, true);
            assert_eq!(profile.len(), EDGE_STEPS + 1);
            assert_eq!(profile[0], [0.0, 0.0]);
            let end = profile[EDGE_STEPS];
            assert!((end[0] - 1.0).abs() < 1e-5 && end[1].abs() < 1e-5);
            let centre =
                profile.iter().map(|p| p[0]).fold(0.0, |sum, u| sum + u) / profile.len() as f32;
            let neck = profile
                .iter()
                .filter(|p| (0.05..0.11).contains(&p[1].abs()))
                .map(|p| (p[0] - centre).abs())
                .fold(f32::MAX, f32::min);
            let head = profile
                .iter()
                .filter(|p| p[1].abs() > 0.15)
                .map(|p| (p[0] - centre).abs())
                .fold(0.0, f32::max);
            assert!(head > neck + 0.02, "seed {seed}: head {head} neck {neck}");
        }
    }

    #[test]
    fn drag_lease_prevents_conflicting_movement_and_snaps() {
        let mut room = room();
        let initial = room.snapshot.as_ref().unwrap().piece(0).unwrap().clone();
        room.acquire("host", 0).unwrap();
        assert!(room.acquire("guest", 0).is_err());
        let x = initial.column as f32 * 200.0;
        let y = initial.row as f32 * 200.0;
        let (pieces, revision) = room.move_piece("host", 0, x, y, true).unwrap();
        assert!(pieces[0].placed);
        assert_eq!(revision, 2);
        assert!(room.acquire("guest", 0).is_err());
    }

    #[test]
    fn selected_groups_are_leased_moved_and_released_atomically() {
        let mut room = room();
        {
            let snapshot = room.snapshot.as_mut().unwrap();
            snapshot.piece_mut(0).unwrap().x = 200.0;
            snapshot.piece_mut(0).unwrap().y = 200.0;
            snapshot.piece_mut(11).unwrap().x = 500.0;
            snapshot.piece_mut(11).unwrap().y = 300.0;
        }

        let (leased, _) = room.acquire_selection("host", &[0, 11, 0]).unwrap();
        assert_eq!(leased.len(), 2, "duplicate selections should be collapsed");
        assert!(
            leased
                .iter()
                .all(|piece| piece.held_by.as_deref() == Some("host"))
        );
        assert!(room.acquire("guest", 11).is_err());

        let (_, _) = room.move_selection("host", 0, 215.0, 212.0, false).unwrap();
        let snapshot = room.snapshot.as_ref().unwrap();
        assert_eq!(
            (snapshot.piece(0).unwrap().x, snapshot.piece(0).unwrap().y),
            (215.0, 212.0)
        );
        assert_eq!(
            (snapshot.piece(11).unwrap().x, snapshot.piece(11).unwrap().y),
            (515.0, 312.0)
        );

        room.move_selection("host", 0, 215.0, 212.0, true).unwrap();
        let snapshot = room.snapshot.as_ref().unwrap();
        assert!(snapshot.piece(0).unwrap().held_by.is_none());
        assert!(snapshot.piece(11).unwrap().held_by.is_none());
        assert!(room.acquire("guest", 0).is_ok());
    }

    #[test]
    fn adjacent_pieces_merge_and_drag_as_a_group() {
        let mut room = room();
        let snapshot = room.snapshot.as_mut().unwrap();
        let left = snapshot.piece_mut(0).unwrap();
        left.x = 100.0;
        left.y = 100.0;
        let right = snapshot.piece_mut(1).unwrap();
        right.x = 320.0;
        right.y = 100.0;
        room.acquire("host", 0).unwrap();
        let (joined, _) = room.move_piece("host", 0, 100.0, 100.0, true).unwrap();
        assert_eq!(joined.len(), 2);
        assert_eq!(joined[0].group_id, joined[1].group_id);
        assert!((joined.iter().find(|p| p.id == 1).unwrap().x - 320.0).abs() < 0.01);

        room.acquire("host", 0).unwrap();
        let (solved, _) = room.move_piece("host", 0, 0.0, 0.0, true).unwrap();
        assert_eq!(solved.len(), 2);
        assert!(solved.iter().all(|piece| piece.placed));
    }

    #[test]
    fn puzzle_completes_when_all_pieces_join_one_group_or_are_placed() {
        let mut snapshot = room().snapshot.clone().unwrap();
        assert!(!snapshot.completed());
        let group = snapshot.pieces[0].group_id;
        for piece in &mut snapshot.pieces {
            piece.group_id = group;
        }
        assert!(snapshot.completed());
        for (index, piece) in snapshot.pieces.iter_mut().enumerate() {
            piece.group_id = index as u32 + 1000;
            piece.placed = true;
        }
        assert!(snapshot.completed());
        snapshot.pieces.clear();
        assert!(!snapshot.completed());
    }

    #[test]
    fn host_restart_rescatters_all_pieces_and_clears_progress() {
        let mut room = room();
        let before: Vec<(f32, f32)> = room
            .snapshot
            .as_ref()
            .unwrap()
            .pieces
            .iter()
            .map(|p| (p.x, p.y))
            .collect();
        // Solve one piece and hold another so there is progress to discard.
        room.acquire("host", 0).unwrap();
        let (cx, cy) = {
            let p = room.snapshot.as_ref().unwrap().piece(0).unwrap();
            (p.column as f32 * 200.0, p.row as f32 * 200.0)
        };
        room.move_piece("host", 0, cx, cy, true).unwrap();
        assert!(room.snapshot.as_ref().unwrap().piece(0).unwrap().placed);
        room.acquire("host", 3).unwrap();
        let revision = room.snapshot.as_ref().unwrap().revision;

        assert!(room.restart("guest", 7).is_err(), "guests cannot restart");
        let (pieces, new_revision) = room.restart("host", 7).unwrap();

        assert_eq!(pieces.len(), 12);
        assert!(new_revision > revision);
        assert!(pieces.iter().all(|p| !p.placed && p.held_by.is_none()));
        assert!(
            pieces.iter().all(|p| p.group_id == p.id),
            "all groups split"
        );
        let after: Vec<(f32, f32)> = pieces.iter().map(|p| (p.x, p.y)).collect();
        assert_ne!(before, after, "a restart should shuffle again");
        assert!(room.acquire("guest", 3).is_ok(), "old leases were dropped");
        assert_ne!(
            room.restart("host", 8)
                .unwrap()
                .0
                .iter()
                .map(|p| (p.x, p.y))
                .collect::<Vec<_>>(),
            after,
            "each restart varies the arrangement"
        );
    }

    #[test]
    fn a_promoted_host_can_restart_even_when_runtime_host_is_stale() {
        let mut room = room();
        // After a server restart the first joiner becomes host by updating the
        // snapshot only.
        room.snapshot.as_mut().unwrap().host_id = "promoted".into();
        room.host_id = None;
        assert!(room.restart("promoted", 3).is_ok());
        assert!(
            room.restart("host", 3).is_err(),
            "the old host no longer can"
        );
    }

    #[test]
    fn playback_is_host_controlled_and_range_checked() {
        let mut room = room();
        assert!(
            room.set_playback("guest", true, 0, 1.0, 0, 1000, 1)
                .is_err()
        );
        assert!(
            room.set_playback("host", true, 0, 1.0, 1000, 500, 1)
                .is_err()
        );
        let (playback, _) = room
            .set_playback("host", true, 0, 1.25, 0, 1000, 42)
            .unwrap();
        assert!(playback.playing);
        assert_eq!(playback.changed_at_ms, 42);
    }

    fn positions(pieces: &[Piece]) -> Vec<(u32, f32, f32)> {
        pieces.iter().map(|p| (p.id, p.x, p.y)).collect()
    }

    fn no_two_overlap(snapshot: &RoomSnapshot) -> bool {
        let (w, h) = (snapshot.cell_width(), snapshot.cell_height());
        snapshot.pieces.iter().enumerate().all(|(i, a)| {
            snapshot.pieces[i + 1..]
                .iter()
                .all(|b| (a.x - b.x).abs() >= w * 0.9 || (a.y - b.y).abs() >= h * 0.9)
        })
    }

    #[test]
    fn edges_first_lays_corners_then_edges_then_interior_into_tray_slots() {
        let mut room = room();
        // Scramble first, so the arrangement has something to fix.
        room.restart("host", 99).unwrap();
        let (moved, revision) = room.rearrange_tray(TrayArrangement::EdgesFirst, 1).unwrap();
        assert_eq!(moved.len(), 12);
        assert!(revision > 0);

        let snapshot = room.snapshot.as_ref().unwrap();
        assert!(no_two_overlap(snapshot));
        let tray = piece_tray_layout(800, 600, 3, 4);
        let slot_of = |piece: &Piece| {
            let col =
                ((piece.x - tray.tray_x) / (PIECE_TRAY_SPACING_CELLS * 200.0)).round() as usize;
            let row = ((piece.y - (tray.tray_y - tray.board_y))
                / (PIECE_TRAY_SPACING_CELLS * 200.0))
                .round() as usize;
            row * tray.columns + col
        };
        let order: Vec<u32> = {
            let mut by_slot: Vec<_> = snapshot.pieces.iter().map(|p| (slot_of(p), p.id)).collect();
            by_slot.sort_unstable();
            by_slot.into_iter().map(|(_, id)| id).collect()
        };
        // 3×4 grid: corners 0, 3, 8, 11; other edges 1, 2, 4, 7, 9, 10; interior 5, 6.
        assert_eq!(order, [0, 3, 8, 11, 1, 2, 4, 7, 9, 10, 5, 6]);
    }

    #[test]
    fn rearranging_leaves_placed_held_and_joined_pieces_alone() {
        let mut room = room();
        {
            let snapshot = room.snapshot.as_mut().unwrap();
            snapshot.piece_mut(0).unwrap().placed = true;
            snapshot.piece_mut(1).unwrap().held_by = Some("guest".into());
            snapshot.piece_mut(5).unwrap().group_id = 6;
        }
        let before = positions(&room.snapshot.as_ref().unwrap().pieces);
        let (moved, _) = room.rearrange_tray(TrayArrangement::Shuffle, 5).unwrap();
        let moved_ids: HashSet<u32> = moved.iter().map(|p| p.id).collect();
        assert_eq!(moved_ids.len(), 8);
        for id in [0, 1, 5, 6] {
            assert!(!moved_ids.contains(&id));
        }
        let snapshot = room.snapshot.as_ref().unwrap();
        for (id, x, y) in before {
            if !moved_ids.contains(&id) {
                let piece = snapshot.piece(id).unwrap();
                assert_eq!((piece.x, piece.y), (x, y), "piece {id} must not move");
            }
        }
        assert!(no_two_overlap(snapshot));
    }

    #[test]
    fn rearranging_avoids_slots_covered_by_a_section_left_in_the_tray() {
        let mut room = room();
        // Park a joined pair right on the tray's first slot.
        let tray = piece_tray_layout(800, 600, 3, 4);
        {
            let snapshot = room.snapshot.as_mut().unwrap();
            for id in [4, 5] {
                let piece = snapshot.piece_mut(id).unwrap();
                piece.group_id = 4;
                piece.x = tray.tray_x + (id - 4) as f32 * 200.0;
                piece.y = tray.tray_y - tray.board_y;
            }
        }
        room.rearrange_tray(TrayArrangement::EdgesFirst, 0).unwrap();
        assert!(no_two_overlap(room.snapshot.as_ref().unwrap()));
    }

    #[test]
    fn shuffle_is_deterministic_per_salt_and_differs_between_salts() {
        let layout = |shuffle_seed| {
            let mut room = room();
            room.rearrange_tray(TrayArrangement::Shuffle, shuffle_seed)
                .unwrap();
            positions(&room.snapshot.as_ref().unwrap().pieces)
        };
        assert_eq!(layout(3), layout(3));
        assert_ne!(layout(3), layout(4));
    }

    #[test]
    fn rearranging_with_nothing_loose_is_refused_without_changing_the_revision() {
        let mut room = room();
        for piece in &mut room.snapshot.as_mut().unwrap().pieces {
            piece.placed = true;
        }
        let revision = room.snapshot.as_ref().unwrap().revision;
        assert!(room.rearrange_tray(TrayArrangement::EdgesFirst, 0).is_err());
        assert_eq!(room.snapshot.as_ref().unwrap().revision, revision);
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    fn snapshot() -> RoomSnapshot {
        RoomSnapshot::create(
            "id".into(),
            "CODE".into(),
            "Title".into(),
            "a".repeat(64),
            "png".into(),
            400,
            300,
            3,
            4,
            "host".into(),
        )
        .unwrap()
    }

    #[test]
    fn progress_counts_placed_and_joined_pieces_and_reaches_100_only_when_complete() {
        let mut room = snapshot();
        assert_eq!(room.progress_percent(), 0);
        for piece in room.pieces.iter_mut().take(3) {
            piece.placed = true;
        }
        assert_eq!(room.progress_percent(), 25);
        // Two loose pieces joined into one group add two settled pieces.
        room.pieces[10].group_id = room.pieces[11].group_id;
        assert_eq!(room.progress_percent(), 41);
        // Eleven of twelve pieces are settled: still short of 100.
        for piece in room.pieces.iter_mut().take(9) {
            piece.placed = true;
        }
        assert_eq!(room.progress_percent(), 91);
        // Even with every piece settled, 100 waits for `completed()`.
        for piece in room.pieces.iter_mut().take(10) {
            piece.placed = true;
        }
        room.pieces[11].group_id = room.pieces[10].group_id;
        assert_eq!(room.progress_percent(), 99);
        for piece in &mut room.pieces {
            piece.placed = true;
        }
        assert_eq!(room.progress_percent(), 100);
    }

    #[test]
    fn a_single_assembled_group_counts_as_complete() {
        let mut room = snapshot();
        for piece in &mut room.pieces {
            piece.group_id = 0;
        }
        assert_eq!(room.progress_percent(), 100);
    }
}

#[cfg(test)]
mod scene_tests {
    use super::*;

    #[test]
    fn joined_pieces_stay_rigid_when_a_move_reaches_the_table_edge() {
        let mut snapshot = RoomSnapshot::create(
            "room".into(),
            "INVITE".into(),
            "Table".into(),
            "a".repeat(64),
            "png".into(),
            400,
            300,
            3,
            4,
            "host".into(),
        )
        .unwrap();
        let scene = snapshot.scene_bounds();
        let tray = piece_tray_layout(400, 300, 3, 4);
        assert!(
            scene.max_x - scene.min_x >= tray.content_width * 5.0
                && scene.max_y - scene.min_y >= tray.content_height * 5.0,
            "the table is several times larger than the board and tray"
        );
        // Join pieces 0 and 1 side by side, then drag them far past the edge.
        let cell_w = snapshot.cell_width();
        for (id, x) in [(0, 100.0), (1, 100.0 + cell_w)] {
            let piece = snapshot.piece_mut(id).unwrap();
            (piece.x, piece.y, piece.group_id) = (x, 50.0, 0);
            piece.held_by = Some("host".into());
        }
        let mut room = RoomRuntime {
            snapshot: Some(snapshot),
            ..RoomRuntime::default()
        };
        room.move_piece("host", 1, 1.0e7, -1.0e7, false).unwrap();
        let snapshot = room.snapshot.as_ref().unwrap();
        let (left, right) = (snapshot.piece(0).unwrap(), snapshot.piece(1).unwrap());
        assert_eq!(right.x - left.x, cell_w, "the pair did not separate");
        assert_eq!(left.y, right.y);
        assert_eq!(right.x + cell_w, scene.max_x, "it stops at the table edge");
        assert_eq!(left.y, scene.min_y);
        // The client-side prediction agrees with what the server allowed.
        assert_eq!(
            snapshot.clamp_move([(left.x, left.y), (right.x, right.y)], 500.0, -500.0),
            (0.0, 0.0)
        );
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::*;

    #[test]
    fn cursor_messages_round_trip_in_the_wire_format() {
        let sent = ClientMessage::Cursor { x: 12.5, y: -3.0 };
        let text = serde_json::to_string(&sent).unwrap();
        assert_eq!(text, r#"{"type":"cursor","x":12.5,"y":-3.0}"#);
        match serde_json::from_str::<ClientMessage>(&text).unwrap() {
            ClientMessage::Cursor { x, y } => assert_eq!((x, y), (12.5, -3.0)),
            other => panic!("unexpected {other:?}"),
        }
        let hidden = serde_json::to_string(&ClientMessage::CursorHidden).unwrap();
        assert_eq!(hidden, r#"{"type":"cursor_hidden"}"#);
        assert!(matches!(
            serde_json::from_str::<ClientMessage>(&hidden).unwrap(),
            ClientMessage::CursorHidden
        ));

        let relayed = ServerMessage::Cursor {
            client_id: "a".into(),
            name: "Ada".into(),
            x: 1.0,
            y: 2.0,
        };
        let text = serde_json::to_string(&relayed).unwrap();
        match serde_json::from_str::<ServerMessage>(&text).unwrap() {
            ServerMessage::Cursor {
                client_id,
                name,
                x,
                y,
            } => assert_eq!(
                (client_id.as_str(), name.as_str(), x, y),
                ("a", "Ada", 1.0, 2.0)
            ),
            other => panic!("unexpected {other:?}"),
        }
        let text = serde_json::to_string(&ServerMessage::CursorHidden {
            client_id: "a".into(),
        })
        .unwrap();
        assert_eq!(text, r#"{"type":"cursor_hidden","client_id":"a"}"#);
    }

    #[test]
    fn a_cursor_must_be_a_finite_point_on_the_table() {
        let snapshot = RoomSnapshot::create(
            "room".into(),
            "INVITE".into(),
            "Table".into(),
            "a".repeat(64),
            "png".into(),
            400,
            300,
            3,
            4,
            "host".into(),
        )
        .unwrap();
        let scene = snapshot.scene_bounds();
        assert!(scene.contains(0.0, 0.0));
        assert!(scene.contains(scene.min_x, scene.max_y));
        assert!(!scene.contains(scene.max_x + 1.0, 0.0));
        assert!(!scene.contains(0.0, scene.min_y - 1.0));
        assert!(!scene.contains(f32::NAN, 0.0));
        assert!(!scene.contains(0.0, f32::INFINITY));
    }
}

#[cfg(test)]
mod chat_tests {
    use super::*;

    #[test]
    fn chat_and_ping_messages_round_trip_in_the_wire_format() {
        let sent = ClientMessage::Chat { text: "hi".into() };
        let text = serde_json::to_string(&sent).unwrap();
        assert_eq!(text, r#"{"type":"chat","text":"hi"}"#);
        assert!(matches!(
            serde_json::from_str::<ClientMessage>(&text).unwrap(),
            ClientMessage::Chat { text } if text == "hi"
        ));
        let ping = serde_json::to_string(&ClientMessage::Ping { x: 1.5, y: 2.0 }).unwrap();
        assert_eq!(ping, r#"{"type":"ping","x":1.5,"y":2.0}"#);

        let line = ChatLine {
            id: 7,
            client_id: "a".into(),
            name: "Ada".into(),
            text: "corner piece?".into(),
            sent_at_ms: 1_700_000_000_000,
        };
        let text = serde_json::to_string(&ServerMessage::Chat(line.clone())).unwrap();
        assert_eq!(
            text,
            r#"{"type":"chat","id":7,"client_id":"a","name":"Ada","text":"corner piece?","sent_at_ms":1700000000000}"#
        );
        match serde_json::from_str::<ServerMessage>(&text).unwrap() {
            ServerMessage::Chat(back) => assert_eq!(back, line),
            other => panic!("unexpected {other:?}"),
        }
        let history = ServerMessage::ChatHistory {
            lines: vec![line.clone()],
        };
        match serde_json::from_str::<ServerMessage>(&serde_json::to_string(&history).unwrap())
            .unwrap()
        {
            ServerMessage::ChatHistory { lines } => assert_eq!(lines, vec![line]),
            other => panic!("unexpected {other:?}"),
        }
        let relayed = ServerMessage::Ping {
            client_id: "a".into(),
            name: "Ada".into(),
            x: 3.0,
            y: 4.0,
        };
        match serde_json::from_str::<ServerMessage>(&serde_json::to_string(&relayed).unwrap())
            .unwrap()
        {
            ServerMessage::Ping { name, x, y, .. } => {
                assert_eq!((name.as_str(), x, y), ("Ada", 3.0, 4.0))
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn chat_text_is_trimmed_stripped_and_bounded() {
        assert_eq!(clean_chat_text("  hello  ").as_deref(), Some("hello"));
        assert_eq!(clean_chat_text("a\nb\tc\u{0}").as_deref(), Some("a b c"));
        assert_eq!(
            clean_chat_text("\u{202e}evil\u{2066}").as_deref(),
            Some("evil")
        );
        assert_eq!(clean_chat_text(""), None);
        // Invisible-only lines are blank; direction marks and soft hyphens go.
        assert_eq!(clean_chat_text("\u{200b}\u{feff}\u{2060}"), None);
        assert_eq!(
            clean_chat_text("a\u{200f}b\u{061c}c\u{00ad}d").as_deref(),
            Some("abcd")
        );
        // The emoji joiner survives.
        assert_eq!(
            clean_chat_text("\u{1f469}\u{200d}\u{1f4bb}").as_deref(),
            Some("\u{1f469}\u{200d}\u{1f4bb}")
        );
        assert_eq!(clean_chat_text(" \n\t "), None);
        assert_eq!(clean_chat_text("\u{7}"), None);
        let long = "é".repeat(MAX_CHAT_CHARS + 50);
        assert_eq!(
            clean_chat_text(&long).unwrap().chars().count(),
            MAX_CHAT_CHARS
        );
        // Cutting never leaves trailing space.
        let spaced = format!("{} tail", "x".repeat(MAX_CHAT_CHARS - 1));
        assert!(!clean_chat_text(&spaced).unwrap().ends_with(' '));
    }
}
