//! Cut styles: how the picture is divided into pieces.
//!
//! Every style keeps the `rows × columns` grid as its piece index: piece
//! `(row, column)` has its home at the grid cell's top-left corner, and its
//! outline is given relative to that anchor, so placing, joining and the tray
//! work the same way for every style. Only the outline and the set of
//! neighbours differ.
//!
//! - `Jigsaw`: the classic rectangular grid with one knob per inner edge.
//!   Its contours come from `piece_outline`'s original code path, unchanged.
//! - `Hexagon`: rows of hexagons. Neighbouring rows are offset by half a
//!   cell, and the line between two rows zig-zags through the hexagons'
//!   points; every inner hexagon has six neighbours.
//! - `Triangle`: rows of alternating up and down triangles with a base two
//!   cells wide. A piece at the left or right border is cut to a trapezoid.
//!
//! Outlines are derived from vertices with stable integer keys. An edge is
//! always generated from its lower-keyed vertex to its higher-keyed one, so
//! both pieces that share it compute the very same samples, one forwards and
//! one backwards. Edges on the picture border are straight.

use serde::{Deserialize, Serialize};

use crate::{KNOB_LEFT, catmull_rom};

/// How a puzzle's picture is cut into pieces. Older rooms and packages have
/// no cut style and load as `Jigsaw`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutStyle {
    #[default]
    Jigsaw,
    Hexagon,
    Triangle,
}

impl CutStyle {
    pub const ALL: [CutStyle; 3] = [CutStyle::Jigsaw, CutStyle::Hexagon, CutStyle::Triangle];

    pub fn label(self) -> &'static str {
        match self {
            CutStyle::Jigsaw => "Classic",
            CutStyle::Hexagon => "Hexagons",
            CutStyle::Triangle => "Triangles",
        }
    }

    /// Cell width over cell height that makes this style's pieces regular:
    /// square for jigsaw pieces, regular hexagons, equilateral triangles.
    pub fn cell_aspect(self) -> f32 {
        match self {
            CutStyle::Jigsaw => 1.0,
            CutStyle::Hexagon => 2.0 / 3.0_f32.sqrt(),
            CutStyle::Triangle => 1.0 / 3.0_f32.sqrt(),
        }
    }

    /// Distance between tray slots in cells, across and down. Each is wide
    /// enough that no two pieces in neighbouring slots overlap.
    pub(crate) fn tray_spacing(self) -> (f32, f32) {
        match self {
            CutStyle::Jigsaw => (
                crate::PIECE_TRAY_SPACING_CELLS,
                crate::PIECE_TRAY_SPACING_CELLS,
            ),
            CutStyle::Hexagon => (2.15, 1.9),
            CutStyle::Triangle => (2.3, 1.6),
        }
    }

    /// Knob depth relative to the edge length.
    fn tab_depth(self) -> f32 {
        match self {
            CutStyle::Jigsaw => 1.0,
            CutStyle::Hexagon => 1.1,
            // Three knobs point into a small triangle; keep them apart.
            CutStyle::Triangle => 0.6,
        }
    }

    /// Inner edges shorter than this fraction of the cell's shorter side
    /// stay straight: a knob would crowd the corner.
    fn min_tab_fraction(self) -> f32 {
        match self {
            CutStyle::Jigsaw => 0.0,
            CutStyle::Hexagon => 0.4,
            CutStyle::Triangle => 0.55,
        }
    }

    /// Samples along an edge with a knob. A multiple of 16, like the
    /// straight edges, so the client's thinned outlines keep every corner.
    fn tab_samples(self) -> usize {
        match self {
            CutStyle::Jigsaw => 80,
            CutStyle::Hexagon => 48,
            CutStyle::Triangle => 64,
        }
    }
}

/// Samples along a straight edge of a hexagon or triangle piece.
const FLAT_SAMPLES: usize = 16;
/// Room for every vertex slot on one horizontal line of the grid.
const LINE_KEYS: u32 = 4096;

/// The grid a puzzle is cut on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tiling {
    pub style: CutStyle,
    pub rows: u16,
    pub columns: u16,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Vertex {
    pub key: u32,
    pub point: [f32; 2],
}

impl Tiling {
    fn cell_width(&self) -> f32 {
        self.width / f32::from(self.columns)
    }

    fn cell_height(&self) -> f32 {
        self.height / f32::from(self.rows)
    }

    /// 1 when cells have the style's ideal shape, falling towards 0 as a
    /// manually chosen grid stretches them. Knobs shrink with it so they
    /// never collide inside a squashed hexagon or triangle.
    fn regularity(&self) -> f32 {
        let ratio = self.cell_width() / self.cell_height() / self.style.cell_aspect();
        if !ratio.is_finite() || ratio <= 0.0 {
            return 0.0;
        }
        ratio.min(1.0 / ratio)
    }

    /// The y of horizontal grid line `line`, exact at the picture border.
    fn line_y(&self, line: u16) -> f32 {
        if line == 0 {
            0.0
        } else if line == self.rows {
            self.height
        } else {
            f32::from(line) * self.cell_height()
        }
    }

    /// Hexagons: x of vertical boundary `k` in `row`, in quarter cells.
    /// Even rows are shifted a quarter cell left, odd rows a quarter right.
    fn hex_quarter(&self, row: u16, k: u16) -> i32 {
        if k == 0 {
            0
        } else if k == self.columns {
            4 * i32::from(self.columns)
        } else {
            4 * i32::from(k) + if row.is_multiple_of(2) { -1 } else { 1 }
        }
    }

    fn quarter_x(&self, quarter: i32) -> f32 {
        if quarter == 0 {
            0.0
        } else if quarter == 4 * i32::from(self.columns) {
            self.width
        } else {
            quarter as f32 * (self.width / (4.0 * f32::from(self.columns)))
        }
    }

    /// Hexagons: the upper end of vertical boundary `k` in `row`.
    fn hex_top(&self, row: u16, k: u16) -> Vertex {
        let x = self.quarter_x(self.hex_quarter(row, k));
        let border = k == 0 || k == self.columns;
        let y = if row == 0 || border {
            self.line_y(row)
        } else {
            self.line_y(row) + self.cell_height() / 6.0
        };
        Vertex {
            key: u32::from(row) * LINE_KEYS + 2 * u32::from(k),
            point: [x, y],
        }
    }

    /// Hexagons: the lower end of vertical boundary `k` in `row`.
    fn hex_bottom(&self, row: u16, k: u16) -> Vertex {
        let line = row + 1;
        let x = self.quarter_x(self.hex_quarter(row, k));
        let border = k == 0 || k == self.columns;
        let y = if line == self.rows || border {
            self.line_y(line)
        } else {
            self.line_y(line) - self.cell_height() / 6.0
        };
        let slot = if border {
            2 * u32::from(k)
        } else {
            2 * u32::from(k) + 1
        };
        Vertex {
            key: u32::from(line) * LINE_KEYS + slot,
            point: [x, y],
        }
    }

    /// Triangles: x of boundary `k` where it meets grid line `line`, in half
    /// cells. Boundaries lean alternately left and right.
    fn tri_half(&self, line: u16, k: u16) -> u16 {
        if k == 0 {
            0
        } else if k == self.columns {
            2 * self.columns
        } else if (k + line).is_multiple_of(2) {
            2 * k + 1
        } else {
            2 * k - 1
        }
    }

    fn tri_vertex(&self, line: u16, half: u16) -> Vertex {
        let x = if half == 0 {
            0.0
        } else if half == 2 * self.columns {
            self.width
        } else {
            f32::from(half) * (self.width / (2.0 * f32::from(self.columns)))
        };
        Vertex {
            key: u32::from(line) * LINE_KEYS + u32::from(half),
            point: [x, self.line_y(line)],
        }
    }

    /// The corners of piece `(row, column)` in picture coordinates, clockwise
    /// on screen starting at the top left.
    pub(crate) fn corners(&self, row: u16, column: u16) -> Vec<Vertex> {
        match self.style {
            CutStyle::Jigsaw => {
                let vertex = |line: u16, k: u16| Vertex {
                    key: u32::from(line) * LINE_KEYS + u32::from(k),
                    point: [
                        if k == self.columns {
                            self.width
                        } else {
                            f32::from(k) * self.cell_width()
                        },
                        self.line_y(line),
                    ],
                };
                vec![
                    vertex(row, column),
                    vertex(row, column + 1),
                    vertex(row + 1, column + 1),
                    vertex(row + 1, column),
                ]
            }
            CutStyle::Hexagon => {
                let (left, right) = (
                    self.hex_quarter(row, column),
                    self.hex_quarter(row, column + 1),
                );
                let mut corners = vec![self.hex_top(row, column)];
                if row > 0 {
                    for k in 1..self.columns {
                        let quarter = self.hex_quarter(row - 1, k);
                        if left < quarter && quarter < right {
                            corners.push(self.hex_bottom(row - 1, k));
                        }
                    }
                }
                corners.push(self.hex_top(row, column + 1));
                corners.push(self.hex_bottom(row, column + 1));
                if row + 1 < self.rows {
                    for k in (1..self.columns).rev() {
                        let quarter = self.hex_quarter(row + 1, k);
                        if left < quarter && quarter < right {
                            corners.push(self.hex_top(row + 1, k));
                        }
                    }
                }
                corners.push(self.hex_bottom(row, column));
                corners
            }
            CutStyle::Triangle => {
                let mut corners = vec![self.tri_vertex(row, self.tri_half(row, column))];
                let top_right = self.tri_vertex(row, self.tri_half(row, column + 1));
                if top_right.key != corners[0].key {
                    corners.push(top_right);
                }
                let bottom_right = self.tri_vertex(row + 1, self.tri_half(row + 1, column + 1));
                corners.push(bottom_right);
                let bottom_left = self.tri_vertex(row + 1, self.tri_half(row + 1, column));
                if bottom_left.key != bottom_right.key {
                    corners.push(bottom_left);
                }
                corners
            }
        }
    }

    /// Pieces sharing an edge with `(row, column)`.
    pub(crate) fn neighbours(&self, row: u16, column: u16) -> Vec<(u16, u16)> {
        let mut out = Vec::with_capacity(6);
        if column > 0 {
            out.push((row, column - 1));
        }
        if column + 1 < self.columns {
            out.push((row, column + 1));
        }
        let other_rows = [row.checked_sub(1), Some(row + 1).filter(|r| *r < self.rows)];
        match self.style {
            CutStyle::Jigsaw => {
                for other in other_rows.into_iter().flatten() {
                    out.push((other, column));
                }
            }
            CutStyle::Hexagon => {
                let (left, right) = (
                    self.hex_quarter(row, column),
                    self.hex_quarter(row, column + 1),
                );
                for other in other_rows.into_iter().flatten() {
                    for k in 0..self.columns {
                        let (l, r) = (self.hex_quarter(other, k), self.hex_quarter(other, k + 1));
                        if l.max(left) < r.min(right) {
                            out.push((other, k));
                        }
                    }
                }
            }
            CutStyle::Triangle => {
                if row > 0 && self.tri_half(row, column) != self.tri_half(row, column + 1) {
                    out.push((row - 1, column));
                }
                if row + 1 < self.rows
                    && self.tri_half(row + 1, column) != self.tri_half(row + 1, column + 1)
                {
                    out.push((row + 1, column));
                }
            }
        }
        out
    }

    /// How many sides of the picture the piece has a straight edge on: 0 for
    /// an interior piece, 1 for an edge piece, 2 for a corner.
    pub(crate) fn border_sides(&self, row: u16, column: u16) -> u8 {
        let corners = self.corners(row, column);
        let mut sides = [false; 4];
        for (index, a) in corners.iter().enumerate() {
            let b = corners[(index + 1) % corners.len()];
            if let Some(side) = self.border_side(a.point, b.point) {
                sides[side] = true;
            }
        }
        sides.iter().filter(|side| **side).count() as u8
    }

    /// Which picture side (top, right, bottom, left) the edge from `a` to
    /// `b` lies on, if it is a border edge.
    fn border_side(&self, a: [f32; 2], b: [f32; 2]) -> Option<usize> {
        if a == b {
            None
        } else if a[1] == 0.0 && b[1] == 0.0 {
            Some(0)
        } else if a[0] == self.width && b[0] == self.width {
            Some(1)
        } else if a[1] == self.height && b[1] == self.height {
            Some(2)
        } else if a[0] == 0.0 && b[0] == 0.0 {
            Some(3)
        } else {
            None
        }
    }

    /// The outline of piece `(row, column)` relative to its cell's top-left
    /// corner. Not used for `Jigsaw`, whose contours keep their original
    /// generator.
    pub(crate) fn outline(&self, puzzle_seed: u32, row: u16, column: u16) -> Vec<[f32; 2]> {
        let corners = self.corners(row, column);
        let anchor = [
            f32::from(column) * self.cell_width(),
            f32::from(row) * self.cell_height(),
        ];
        let mut points = Vec::new();
        for (index, a) in corners.iter().enumerate() {
            let b = corners[(index + 1) % corners.len()];
            let edge = self.edge_points(puzzle_seed, *a, b);
            for point in &edge[..edge.len() - 1] {
                points.push([point[0] - anchor[0], point[1] - anchor[1]]);
            }
        }
        points
    }

    /// The samples of the edge from `a` to `b`, both ends included.
    fn edge_points(&self, puzzle_seed: u32, a: Vertex, b: Vertex) -> Vec<[f32; 2]> {
        let (start, end, reversed) = if a.key < b.key {
            (a, b, false)
        } else {
            (b, a, true)
        };
        let (dx, dy) = (end.point[0] - start.point[0], end.point[1] - start.point[1]);
        let length = dx.hypot(dy);
        let shorter_side = self.cell_width().min(self.cell_height());
        let knob = self.border_side(a.point, b.point).is_none()
            && length >= self.style.min_tab_fraction() * shorter_side;
        let profile = if knob {
            tab_profile(
                edge_seed(puzzle_seed, start.key, end.key),
                self.style.tab_depth() * self.regularity(),
                self.style.tab_samples(),
            )
        } else {
            (0..=FLAT_SAMPLES)
                .map(|step| [step as f32 / FLAT_SAMPLES as f32, 0.0])
                .collect()
        };
        let normal = if length > 0.0 {
            [-dy / length, dx / length]
        } else {
            [0.0, 0.0]
        };
        let mut points: Vec<[f32; 2]> = profile
            .iter()
            .map(|[u, v]| {
                [
                    start.point[0] + dx * u + normal[0] * v * length,
                    start.point[1] + dy * u + normal[1] * v * length,
                ]
            })
            .collect();
        // Pin the ends to the exact corners so neighbouring edges meet.
        let last = points.len() - 1;
        points[0] = start.point;
        points[last] = end.point;
        if reversed {
            points.reverse();
        }
        points
    }
}

fn edge_seed(puzzle_seed: u32, a: u32, b: u32) -> u32 {
    let mut hash = puzzle_seed ^ 0x2c1b_3c6d;
    for value in [a, b] {
        hash = (hash ^ value).wrapping_mul(0x0100_0193);
        hash ^= hash >> 15;
    }
    hash
}

/// `samples + 1` points `[u, v]` along an edge of unit length: a knob whose
/// direction, size and position vary with the seed, `depth` times the
/// classic knob's proportions.
fn tab_profile(seed: u32, depth: f32, samples: usize) -> Vec<[f32; 2]> {
    let mixed = seed.wrapping_mul(0x7feb_352d).wrapping_add(0x846c_a68b);
    let sign = if mixed & 1 == 0 { 1.0 } else { -1.0 };
    let shift = (((mixed >> 8) & 0xff) as f32 / 255.0 - 0.5) * 0.08;
    let size = 0.92 + ((mixed >> 16) & 0xff) as f32 / 255.0 * 0.16;
    let scaled: Vec<[f32; 2]> = KNOB_LEFT
        .iter()
        .enumerate()
        .map(|(index, key)| {
            if index == 0 {
                *key
            } else {
                [0.5 + (key[0] - 0.5) * size, key[1] * size * sign * depth]
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
    let mut points = Vec::with_capacity(samples + 1);
    for step in 0..samples {
        let along = (step * segments) as f32 / samples as f32;
        let segment = (along as usize).min(segments - 1);
        let t = along - segment as f32;
        let p0 = keys[segment.saturating_sub(1)];
        let p1 = keys[segment];
        let p2 = keys[segment + 1];
        let p3 = keys[(segment + 2).min(last)];
        points.push(catmull_rom(p0, p1, p2, p3, t));
    }
    points.push(keys[last]);
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// One piece's view of an edge: the piece and the samples it traces.
    type EdgeSide = ((u16, u16), Vec<[f32; 2]>);

    const STYLES: [CutStyle; 2] = [CutStyle::Hexagon, CutStyle::Triangle];

    /// Grids to check: small, odd, preset-like and the largest allowed.
    fn grids(style: CutStyle) -> Vec<Tiling> {
        let mut out = Vec::new();
        for (rows, columns, width, height) in [
            (2, 2, 400.0, 300.0),
            (2, 3, 640.0, 360.0),
            (3, 4, 800.0, 600.0),
            (5, 7, 1000.0, 700.0),
            (6, 9, 1920.0, 1080.0),
            (20, 20, 1600.0, 1200.0),
            (13, 30, 1920.0, 1080.0),
            (6, 18, 1920.0, 1080.0),
            (14, 28, 1600.0, 1600.0),
            // Badly stretched manual grids.
            (30, 2, 1920.0, 1080.0),
            (2, 30, 1080.0, 1920.0),
        ] {
            out.push(Tiling {
                style,
                rows,
                columns,
                width,
                height,
            });
        }
        out
    }

    fn signed_area(points: &[[f32; 2]]) -> f64 {
        let mut sum = 0.0;
        for (index, a) in points.iter().enumerate() {
            let b = points[(index + 1) % points.len()];
            sum += f64::from(a[0]) * f64::from(b[1]) - f64::from(b[0]) * f64::from(a[1]);
        }
        sum * 0.5
    }

    fn absolute_outline(tiling: &Tiling, seed: u32, row: u16, column: u16) -> Vec<[f32; 2]> {
        let (x, y) = (
            f32::from(column) * tiling.cell_width(),
            f32::from(row) * tiling.cell_height(),
        );
        tiling
            .outline(seed, row, column)
            .into_iter()
            .map(|point| [point[0] + x, point[1] + y])
            .collect()
    }

    fn segments_cross(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> bool {
        let orient = |p: [f32; 2], q: [f32; 2], r: [f32; 2]| {
            f64::from(q[0] - p[0]) * f64::from(r[1] - p[1])
                - f64::from(q[1] - p[1]) * f64::from(r[0] - p[0])
        };
        let (d1, d2) = (orient(c, d, a), orient(c, d, b));
        let (d3, d4) = (orient(a, b, c), orient(a, b, d));
        d1 * d2 < 0.0 && d3 * d4 < 0.0
    }

    #[test]
    fn pieces_cover_the_picture_without_gaps_or_overlaps() {
        for style in STYLES {
            for tiling in grids(style) {
                let picture = f64::from(tiling.width) * f64::from(tiling.height);
                let (mut corner_sum, mut outline_sum) = (0.0, 0.0);
                for row in 0..tiling.rows {
                    for column in 0..tiling.columns {
                        let corners: Vec<_> = tiling
                            .corners(row, column)
                            .iter()
                            .map(|vertex| vertex.point)
                            .collect();
                        let area = signed_area(&corners);
                        assert!(area > 0.0, "{style:?} {row},{column} is not clockwise");
                        corner_sum += area;
                        let outline = absolute_outline(&tiling, 7, row, column);
                        let area = signed_area(&outline);
                        assert!(area > 0.0);
                        outline_sum += area;
                    }
                }
                let tolerance = picture * 1e-5;
                assert!(
                    (corner_sum - picture).abs() < tolerance,
                    "{style:?} {tiling:?}"
                );
                assert!(
                    (outline_sum - picture).abs() < tolerance,
                    "{style:?} {tiling:?}"
                );
            }
        }
    }

    #[test]
    fn neighbours_share_identical_edges_and_borders_are_straight() {
        for style in STYLES {
            for tiling in grids(style) {
                // Every edge by its vertex keys: the pieces on it and the
                // samples each traces.
                let mut edges: HashMap<(u32, u32), Vec<EdgeSide>> = HashMap::new();
                for row in 0..tiling.rows {
                    for column in 0..tiling.columns {
                        let corners = tiling.corners(row, column);
                        for (index, a) in corners.iter().enumerate() {
                            let b = corners[(index + 1) % corners.len()];
                            let points = tiling.edge_points(99, *a, b);
                            assert_eq!(points.len() % 16, 1, "{style:?}: corners survive thinning");
                            if let Some(side) = tiling.border_side(a.point, b.point) {
                                let on_side = |p: &[f32; 2]| match side {
                                    0 => p[1] == 0.0,
                                    1 => p[0] == tiling.width,
                                    2 => p[1] == tiling.height,
                                    _ => p[0] == 0.0,
                                };
                                assert!(points.iter().all(on_side), "{style:?} border is straight");
                            }
                            edges
                                .entry((a.key.min(b.key), a.key.max(b.key)))
                                .or_default()
                                .push(((row, column), points));
                        }
                    }
                }
                let mut sharing: HashSet<((u16, u16), (u16, u16))> = HashSet::new();
                for (key, owners) in &edges {
                    match owners.as_slice() {
                        [(piece, points)] => {
                            let a = points[0];
                            let b = points[points.len() - 1];
                            assert!(
                                tiling.border_side(a, b).is_some(),
                                "{style:?} {piece:?}: inner edge {key:?} has one side"
                            );
                        }
                        [(first, a), (second, b)] => {
                            assert_ne!(first, second);
                            let reversed: Vec<_> = b.iter().rev().copied().collect();
                            assert_eq!(a, &reversed, "{style:?} {first:?}/{second:?}");
                            sharing.insert((*first, *second));
                            sharing.insert((*second, *first));
                        }
                        other => panic!("{style:?}: edge {key:?} has {} sides", other.len()),
                    }
                }
                let mut listed = HashSet::new();
                for row in 0..tiling.rows {
                    for column in 0..tiling.columns {
                        for other in tiling.neighbours(row, column) {
                            listed.insert(((row, column), other));
                        }
                    }
                }
                assert_eq!(
                    listed, sharing,
                    "{style:?} {tiling:?}: neighbours follow edges"
                );
                for &(a, b) in &listed {
                    assert!(listed.contains(&(b, a)), "neighbours are symmetric");
                }
            }
        }
    }

    #[test]
    fn inner_hexagons_have_six_neighbours_and_triangles_three() {
        let hex = grids(CutStyle::Hexagon)[3];
        assert_eq!(hex.neighbours(2, 3).len(), 6);
        let triangle = grids(CutStyle::Triangle)[3];
        assert_eq!(triangle.neighbours(2, 3).len(), 3);
        // A hexagon in an even row meets the row below at columns c-1 and c.
        let below: Vec<_> = hex
            .neighbours(2, 3)
            .into_iter()
            .filter(|(row, _)| *row == 3)
            .collect();
        assert_eq!(below, vec![(3, 2), (3, 3)]);
    }

    #[test]
    fn outlines_are_simple_polygons() {
        for tiling in STYLES.into_iter().flat_map(grids) {
            let style = tiling.style;
            for row in 0..tiling.rows {
                for column in 0..tiling.columns {
                    let outline = absolute_outline(&tiling, 3, row, column);
                    let n = outline.len();
                    for i in 0..n {
                        for j in i + 2..n {
                            if i == 0 && j == n - 1 {
                                continue;
                            }
                            assert!(
                                !segments_cross(
                                    outline[i],
                                    outline[(i + 1) % n],
                                    outline[j],
                                    outline[(j + 1) % n]
                                ),
                                "{style:?} {tiling:?} {row},{column} crosses itself"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn outlines_are_deterministic_and_vary_with_the_seed() {
        for style in STYLES {
            let tiling = grids(style)[3];
            assert_eq!(tiling.outline(5, 2, 3), tiling.outline(5, 2, 3));
            assert_ne!(tiling.outline(5, 2, 3), tiling.outline(6, 2, 3));
        }
    }

    #[test]
    fn border_sides_find_corners_and_edges() {
        for style in [CutStyle::Jigsaw, CutStyle::Hexagon, CutStyle::Triangle] {
            for tiling in grids(style) {
                let (last_row, last_column) = (tiling.rows - 1, tiling.columns - 1);
                for row in 0..tiling.rows {
                    for column in 0..tiling.columns {
                        let sides = tiling.border_sides(row, column);
                        let corner =
                            (row == 0 || row == last_row) && (column == 0 || column == last_column);
                        assert_eq!(sides == 2, corner, "{style:?} {row},{column}");
                        if column == 0 || column == last_column {
                            assert!(sides >= 1);
                        }
                        if row > 0 && row < last_row && column > 0 && column < last_column {
                            assert_eq!(sides, 0);
                        }
                    }
                }
            }
        }
        // An upward triangle in the top row only touches the border with
        // its tip, so it is not an edge piece.
        let triangle = grids(CutStyle::Triangle)[3];
        let top: Vec<u8> = (1..6)
            .map(|column| triangle.border_sides(0, column))
            .collect();
        assert!(top.contains(&0) && top.contains(&1), "{top:?}");
    }

    #[test]
    fn tray_slots_are_wide_enough_for_every_piece() {
        for style in STYLES {
            for tiling in grids(style) {
                // Only grids close to the style's own cell shape: stretched
                // manual grids may overlap a little in the tray.
                let aspect = tiling.cell_width() / tiling.cell_height();
                if (aspect / style.cell_aspect() - 1.0).abs() > 0.35 {
                    continue;
                }
                let (mut min_x, mut max_x, mut min_y, mut max_y) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
                for row in 0..tiling.rows {
                    for column in 0..tiling.columns {
                        for point in tiling.outline(11, row, column) {
                            min_x = min_x.min(point[0] / tiling.cell_width());
                            max_x = max_x.max(point[0] / tiling.cell_width());
                            min_y = min_y.min(point[1] / tiling.cell_height());
                            max_y = max_y.max(point[1] / tiling.cell_height());
                        }
                    }
                }
                let (across, down) = style.tray_spacing();
                assert!(max_x - min_x <= across, "{style:?} {tiling:?}");
                assert!(max_y - min_y <= down, "{style:?} {tiling:?}");
            }
        }
    }
}
