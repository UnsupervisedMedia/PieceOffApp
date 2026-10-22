//! Piece holders: a player's own sorting areas on the table (protocol v12).
//!
//! A holder is a rectangle on the table that belongs to one player, by
//! display name (solve credits key by name too). Name claims (`claims`) keep
//! a name with the device that first used it in the room, so nobody else can
//! join under it and take over its holders. A loose piece whose centre lies inside someone else's holder is
//! reserved: other players can see it but cannot pick it up. The owner moves
//! pieces in and out freely, and anyone may drop a piece into a holder (it is
//! then reserved for the owner). Placed pieces are never reserved.

use crate::{Piece, RoomRuntime, RoomSnapshot, ServerMessage, clean_chat_text};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Holders one player may own at a time.
pub const MAX_HOLDERS_PER_PLAYER: usize = 4;
/// Holders a room may hold at a time.
pub const MAX_HOLDERS_PER_ROOM: usize = 16;
/// Smallest and largest holder side, in puzzle cells.
pub const HOLDER_MIN_CELLS: f32 = 2.0;
pub const HOLDER_MAX_CELLS: f32 = 12.0;
/// Size of a new holder made with the button or key, in cells.
pub const HOLDER_DEFAULT_CELLS: (f32, f32) = (5.0, 4.0);
/// How long a holder outlives its owner's departure, on the server clock.
/// A player who comes back under the same name within this keeps it.
pub const HOLDER_GRACE_MS: u64 = 10 * 60 * 1000;
pub const MAX_HOLDER_LABEL_CHARS: usize = 24;
/// Free space kept around the board and the tray, in cells, so piece tabs
/// on their edges never fall into a holder.
const HOLDER_CLEARANCE_CELLS: f32 = 0.3;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PieceHolder {
    pub id: u32,
    /// The owner's display name.
    pub owner: String,
    pub label: String,
    /// Top-left corner and size in board coordinates (source-image pixels
    /// from the board's top-left).
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Server clock (ms since the Unix epoch) when the server noticed the
    /// owner was gone; `None` while they are in the room.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_away_since_ms: Option<u64>,
}

impl PieceHolder {
    /// Whether a point lies inside, counting the top and left edges but not
    /// the bottom and right, so a point is never inside two touching holders.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    fn overlaps(&self, x: f32, y: f32, width: f32, height: f32) -> bool {
        rects_overlap(
            (self.x, self.y, self.width, self.height),
            (x, y, width, height),
        )
    }
}

fn rects_overlap(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

/// Tidy a holder label like a chat line, cut to `MAX_HOLDER_LABEL_CHARS`.
/// An empty label becomes "Holder".
pub fn clean_holder_label(label: &str) -> String {
    clean_chat_text(label)
        .map(|text| {
            text.chars()
                .take(MAX_HOLDER_LABEL_CHARS)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "Holder".into())
}

impl RoomSnapshot {
    /// The holder whose area contains this piece's centre, if the piece is
    /// loose. "Inside" is decided by the centre of the piece's cell alone.
    pub fn holder_of(&self, piece: &Piece) -> Option<&PieceHolder> {
        if piece.placed {
            return None;
        }
        let cx = piece.x + self.cell_width() * 0.5;
        let cy = piece.y + self.cell_height() * 0.5;
        self.holders.iter().find(|holder| holder.contains(cx, cy))
    }

    /// The holder that keeps `player` (a display name; `None` for nobody)
    /// from picking up `group_id`: any loose member lying in a holder owned
    /// by someone else reserves the whole group.
    pub fn reserved_from(&self, group_id: u32, player: Option<&str>) -> Option<&PieceHolder> {
        self.pieces
            .iter()
            .filter(|piece| piece.group_id == group_id)
            .filter_map(|piece| self.holder_of(piece))
            .find(|holder| Some(holder.owner.as_str()) != player)
    }

    /// Why a holder may not occupy this rectangle, or `Ok` when it may.
    /// `ignore` is the holder being moved, which may overlap its old self.
    pub fn check_holder_rect(
        &self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        ignore: Option<u32>,
    ) -> Result<(), String> {
        if ![x, y, width, height].iter().all(|value| value.is_finite()) {
            return Err("Holder coordinates must be finite".into());
        }
        let (cw, ch) = (self.cell_width(), self.cell_height());
        let side = |length: f32, cell: f32| {
            (HOLDER_MIN_CELLS * cell - 0.01..=HOLDER_MAX_CELLS * cell + 0.01).contains(&length)
        };
        if !side(width, cw) || !side(height, ch) {
            return Err(format!(
                "A holder must be between {HOLDER_MIN_CELLS} and {HOLDER_MAX_CELLS} pieces on each side"
            ));
        }
        let scene = self.scene_bounds();
        if x < scene.min_x || y < scene.min_y || x + width > scene.max_x || y + height > scene.max_y
        {
            return Err("A holder must lie on the table".into());
        }
        let (pad_x, pad_y) = (HOLDER_CLEARANCE_CELLS * cw, HOLDER_CLEARANCE_CELLS * ch);
        let tray = self.tray_layout();
        let board = (
            -pad_x,
            -pad_y,
            self.width as f32 + 2.0 * pad_x,
            self.height as f32 + 2.0 * pad_y,
        );
        let tray = (
            tray.tray_x - pad_x,
            tray.tray_y - tray.board_y - pad_y,
            tray.tray_width + 2.0 * pad_x,
            tray.tray_height + 2.0 * pad_y,
        );
        let rect = (x, y, width, height);
        if rects_overlap(rect, board) || rects_overlap(rect, tray) {
            return Err("Holders go on the open table, not on the board or the tray".into());
        }
        if self
            .holders
            .iter()
            .filter(|holder| Some(holder.id) != ignore)
            .any(|holder| holder.overlaps(x, y, width, height))
        {
            return Err("That overlaps another holder".into());
        }
        Ok(())
    }

    /// The free spot nearest to `(near_x, near_y)` where a new holder of this
    /// size fits, as its top-left corner. Searches outward one cell at a time.
    pub fn free_holder_spot(
        &self,
        width: f32,
        height: f32,
        near_x: f32,
        near_y: f32,
    ) -> Option<(f32, f32)> {
        let (cw, ch) = (self.cell_width(), self.cell_height());
        if !(near_x.is_finite() && near_y.is_finite() && cw > 0.0 && ch > 0.0) {
            return None;
        }
        let scene = self.scene_bounds();
        let reach = ((scene.max_x - scene.min_x) / cw).max((scene.max_y - scene.min_y) / ch);
        let reach = reach.ceil().min(2_000.0) as i32;
        let (start_x, start_y) = (near_x - width * 0.5, near_y - height * 0.5);
        let fits = |i: i32, j: i32| {
            let (x, y) = (start_x + i as f32 * cw, start_y + j as f32 * ch);
            self.check_holder_rect(x, y, width, height, None)
                .is_ok()
                .then_some((x, y, i * i + j * j))
        };
        for ring in 0..=reach {
            let mut best: Option<(f32, f32, i32)> = None;
            for i in -ring..=ring {
                for j in [-ring, ring] {
                    for (a, b) in [(i, j), (j, i)] {
                        if let Some(found) = fits(a, b)
                            && best.is_none_or(|current| found.2 < current.2)
                        {
                            best = Some(found);
                        }
                    }
                }
            }
            if let Some((x, y, _)) = best {
                return Some((x, y));
            }
        }
        None
    }
}

impl RoomRuntime {
    fn holder_owner_name(&self, client_id: &str) -> Result<String, String> {
        self.names
            .get(client_id)
            .cloned()
            .ok_or_else(|| "Join the room before using holders".into())
    }

    /// Make a holder for `client_id` at a free, valid spot.
    pub fn create_holder(
        &mut self,
        client_id: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        label: &str,
    ) -> Result<u64, String> {
        let owner = self.holder_owner_name(client_id)?;
        let snapshot = self.snapshot.as_mut().ok_or("Room is not initialized")?;
        if snapshot.holders.len() >= MAX_HOLDERS_PER_ROOM {
            return Err(format!(
                "A room can have at most {MAX_HOLDERS_PER_ROOM} holders"
            ));
        }
        if snapshot
            .holders
            .iter()
            .filter(|holder| holder.owner == owner)
            .count()
            >= MAX_HOLDERS_PER_PLAYER
        {
            return Err(format!(
                "You can have at most {MAX_HOLDERS_PER_PLAYER} holders"
            ));
        }
        snapshot.check_holder_rect(x, y, width, height, None)?;
        let id = snapshot
            .holders
            .iter()
            .map(|holder| holder.id + 1)
            .max()
            .unwrap_or(1);
        snapshot.holders.push(PieceHolder {
            id,
            owner,
            label: clean_holder_label(label),
            x,
            y,
            width,
            height,
            owner_away_since_ms: None,
        });
        snapshot.revision += 1;
        Ok(snapshot.revision)
    }

    /// Move, resize or rename the caller's own holder. Moving it carries the
    /// loose groups inside it along, except a group someone is dragging, a
    /// group another player's holder reserves (it straddles both), and a
    /// group that would land in another player's holder. Returns the pieces
    /// that moved.
    #[allow(clippy::too_many_arguments)]
    pub fn update_holder(
        &mut self,
        client_id: &str,
        holder_id: u32,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        label: &str,
    ) -> Result<(Vec<Piece>, u64), String> {
        let owner = self.holder_owner_name(client_id)?;
        let snapshot = self.snapshot.as_mut().ok_or("Room is not initialized")?;
        let old = snapshot
            .holders
            .iter()
            .find(|holder| holder.id == holder_id)
            .cloned()
            .ok_or("That holder is gone")?;
        if old.owner != owner {
            return Err("Only its owner can change a holder".into());
        }
        snapshot.check_holder_rect(x, y, width, height, Some(holder_id))?;
        let (dx, dy) = (x - old.x, y - old.y);
        let mut moved = Vec::new();
        if dx != 0.0 || dy != 0.0 {
            let groups: HashSet<u32> = snapshot
                .pieces
                .iter()
                .filter(|piece| {
                    snapshot
                        .holder_of(piece)
                        .is_some_and(|holder| holder.id == holder_id)
                })
                .map(|piece| piece.group_id)
                .collect();
            let mut groups: Vec<u32> = groups
                .into_iter()
                .filter(|group| {
                    snapshot
                        .pieces
                        .iter()
                        .filter(|piece| piece.group_id == *group)
                        .all(|piece| !piece.placed && piece.held_by.is_none())
                        && snapshot.reserved_from(*group, Some(&owner)).is_none()
                })
                .collect();
            groups.sort_unstable();
            let (half_w, half_h) = (snapshot.cell_width() * 0.5, snapshot.cell_height() * 0.5);
            for group in groups {
                let (gx, gy) = snapshot.clamp_move(
                    snapshot
                        .pieces
                        .iter()
                        .filter(|piece| piece.group_id == group)
                        .map(|piece| (piece.x, piece.y)),
                    dx,
                    dy,
                );
                let lands_in_another = snapshot
                    .pieces
                    .iter()
                    .filter(|piece| piece.group_id == group)
                    .any(|piece| {
                        let (cx, cy) = (piece.x + gx + half_w, piece.y + gy + half_h);
                        snapshot.holders.iter().any(|holder| {
                            holder.id != holder_id
                                && holder.owner != owner
                                && holder.contains(cx, cy)
                        })
                    });
                if lands_in_another {
                    continue;
                }
                for piece in &mut snapshot.pieces {
                    if piece.group_id == group {
                        piece.x += gx;
                        piece.y += gy;
                        moved.push(piece.clone());
                    }
                }
            }
        }
        let holder = snapshot
            .holders
            .iter_mut()
            .find(|holder| holder.id == holder_id)
            .ok_or("That holder is gone")?;
        holder.x = x;
        holder.y = y;
        holder.width = width;
        holder.height = height;
        holder.label = clean_holder_label(label);
        snapshot.revision += 1;
        Ok((moved, snapshot.revision))
    }

    /// Remove a holder; its pieces stay where they are, free for anyone. The
    /// owner and the host may do this.
    pub fn delete_holder(&mut self, client_id: &str, holder_id: u32) -> Result<u64, String> {
        let name = self.names.get(client_id).cloned();
        let snapshot = self.snapshot.as_mut().ok_or("Room is not initialized")?;
        let index = snapshot
            .holders
            .iter()
            .position(|holder| holder.id == holder_id)
            .ok_or("That holder is gone")?;
        let is_owner = name.as_deref() == Some(snapshot.holders[index].owner.as_str());
        if !is_owner && snapshot.host_id != client_id {
            return Err("Only its owner or the host can remove a holder".into());
        }
        snapshot.holders.remove(index);
        snapshot.revision += 1;
        Ok(snapshot.revision)
    }

    /// Keep holder ownership in step with who is in the room. Holders of a
    /// player who has gone are kept for `HOLDER_GRACE_MS`, so coming back
    /// under the same name reclaims them; after that they are removed and
    /// their pieces stay where they lie. Hosts call this on a timer. Returns
    /// the event to broadcast (and persist) when an owner went away, came
    /// back, or had their holders released.
    pub fn tick_holders(&mut self, now_ms: u64) -> Option<ServerMessage> {
        let present: HashSet<&str> = self.names.values().map(String::as_str).collect();
        let snapshot = self.snapshot.as_mut()?;
        let mut changed = false;
        for holder in &mut snapshot.holders {
            if present.contains(holder.owner.as_str()) {
                changed |= holder.owner_away_since_ms.take().is_some();
            } else if holder.owner_away_since_ms.is_none() {
                holder.owner_away_since_ms = Some(now_ms);
                changed = true;
            }
        }
        let before = snapshot.holders.len();
        snapshot.holders.retain(|holder| {
            holder
                .owner_away_since_ms
                .is_none_or(|since| now_ms.saturating_sub(since) < HOLDER_GRACE_MS)
        });
        if !changed && snapshot.holders.len() == before {
            return None;
        }
        snapshot.revision += 1;
        Some(ServerMessage::PiecesChanged {
            pieces: Vec::new(),
            revision: snapshot.revision,
            stats: None,
            holders: Some(snapshot.holders.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClientMessage, TrayArrangement};
    use std::collections::HashMap;

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
            names: HashMap::from([
                ("host".into(), "Host".into()),
                ("ada".into(), "Ada".into()),
                ("bo".into(), "Bo".into()),
            ]),
            host_id: Some("host".into()),
            ..RoomRuntime::default()
        }
    }

    fn snapshot(room: &RoomRuntime) -> &RoomSnapshot {
        room.snapshot.as_ref().unwrap()
    }

    /// A holder spot below the board (cells are 200×200; the tray starts at
    /// x = 940 and reaches down to y = 1390).
    const SPOT: (f32, f32, f32, f32) = (-200.0, 800.0, 1000.0, 800.0);

    fn create(
        room: &mut RoomRuntime,
        who: &str,
        rect: (f32, f32, f32, f32),
    ) -> Result<u32, String> {
        let out = room.apply(
            who,
            ClientMessage::CreateHolder {
                x: rect.0,
                y: rect.1,
                width: rect.2,
                height: rect.3,
                label: "Edges".into(),
            },
            0,
        )?;
        assert!(out.persist);
        let ServerMessage::PiecesChanged {
            holders: Some(holders),
            ..
        } = out.event
        else {
            panic!("a holder change is announced with the holders");
        };
        Ok(holders.last().unwrap().id)
    }

    /// Drop piece `id` with its centre at `(cx, cy)`.
    fn drop_at(room: &mut RoomRuntime, who: &str, id: u32, cx: f32, cy: f32) {
        room.apply(who, ClientMessage::DragStart { piece_id: id }, 0)
            .unwrap();
        room.apply(
            who,
            ClientMessage::DragEnd {
                piece_id: id,
                x: cx - 100.0,
                y: cy - 100.0,
            },
            0,
        )
        .unwrap();
    }

    #[test]
    fn a_piece_in_someone_elses_holder_is_reserved_for_its_owner() {
        let mut room = room();
        let holder = create(&mut room, "ada", SPOT).unwrap();
        // Anyone may drop a piece in; it then belongs to the holder.
        drop_at(&mut room, "bo", 5, 300.0, 1000.0);
        assert_eq!(
            snapshot(&room)
                .holder_of(snapshot(&room).piece(5).unwrap())
                .map(|h| h.id),
            Some(holder)
        );
        let err = room
            .apply("bo", ClientMessage::DragStart { piece_id: 5 }, 0)
            .unwrap_err();
        assert_eq!(err, "That piece is in Ada's holder");
        let err = room
            .apply(
                "host",
                ClientMessage::DragStartSelection {
                    piece_ids: vec![1, 5],
                },
                0,
            )
            .unwrap_err();
        assert_eq!(err, "That piece is in Ada's holder");
        assert!(snapshot(&room).pieces.iter().all(|p| p.held_by.is_none()));
        // The owner moves it out, and then it is anyone's.
        drop_at(&mut room, "ada", 5, -900.0, 300.0);
        assert!(
            snapshot(&room)
                .holder_of(snapshot(&room).piece(5).unwrap())
                .is_none()
        );
        room.apply("bo", ClientMessage::DragStart { piece_id: 5 }, 0)
            .unwrap();
    }

    #[test]
    fn inside_means_the_piece_centre_and_placed_pieces_are_never_reserved() {
        let mut room = room();
        create(&mut room, "ada", SPOT).unwrap();
        // Centre just above the holder's top edge: outside.
        drop_at(&mut room, "ada", 5, 300.0, 799.0);
        room.apply("bo", ClientMessage::DragStart { piece_id: 5 }, 0)
            .unwrap();
        let mut placed = snapshot(&room).piece(0).unwrap().clone();
        placed.placed = true;
        placed.x = 300.0;
        placed.y = 900.0;
        assert!(snapshot(&room).holder_of(&placed).is_none());
    }

    #[test]
    fn a_group_with_one_member_in_a_holder_is_reserved_whole() {
        let mut room = room();
        create(&mut room, "ada", SPOT).unwrap();
        let snap = room.snapshot.as_mut().unwrap();
        // Pieces 0 and 1 joined, 1's centre inside, 0's outside.
        for (id, x) in [(0, -400.0), (1, -200.0)] {
            let piece = snap.piece_mut(id).unwrap();
            piece.group_id = 0;
            piece.x = x;
            piece.y = 800.0;
        }
        let err = room
            .apply("bo", ClientMessage::DragStart { piece_id: 0 }, 0)
            .unwrap_err();
        assert!(err.contains("Ada's holder"));
    }

    #[test]
    fn holders_are_validated_and_limited() {
        let mut room = room();
        let bad = [
            (f32::NAN, 800.0, 1000.0, 800.0),
            (0.0, 800.0, 200.0, 800.0),  // one cell wide
            (0.0, 800.0, 2600.0, 800.0), // thirteen cells
            (0.0, 100.0, 1000.0, 800.0), // on the board
            (0.0, 1e7, 1000.0, 800.0),   // off the table
        ];
        for rect in bad {
            assert!(create(&mut room, "ada", rect).is_err(), "{rect:?}");
        }
        create(&mut room, "ada", SPOT).unwrap();
        assert_eq!(
            create(&mut room, "bo", (-700.0, 1000.0, 1000.0, 800.0)).unwrap_err(),
            "That overlaps another holder"
        );
        // Touching is fine; a fifth per player is not.
        for index in 1..4 {
            create(
                &mut room,
                "ada",
                (-200.0, 800.0 + 800.0 * index as f32, 1000.0, 800.0),
            )
            .unwrap();
        }
        assert!(
            create(&mut room, "ada", (1000.0, 800.0, 1000.0, 800.0))
                .unwrap_err()
                .contains("at most 4")
        );
        // Nobody unnamed makes holders.
        assert!(create(&mut room, "stranger", (1000.0, 800.0, 1000.0, 800.0)).is_err());
        let snap = snapshot(&room);
        assert_eq!(snap.holders.len(), 4);
        assert!(snap.holders.iter().all(|holder| holder.label == "Edges"));
    }

    #[test]
    fn a_room_holds_at_most_sixteen_holders() {
        let mut room = room();
        for index in 0..8 {
            room.names.insert(format!("p{index}"), format!("P{index}"));
        }
        let mut made = 0;
        'outer: for column in 0..8 {
            for row in 0..4 {
                let who = format!("p{column}");
                let rect = (
                    -5000.0 + 1000.0 * column as f32,
                    2000.0 + 800.0 * row as f32,
                    1000.0,
                    800.0,
                );
                match create(&mut room, &who, rect) {
                    Ok(_) => made += 1,
                    Err(err) => {
                        assert!(err.contains("at most 16"), "{err}");
                        break 'outer;
                    }
                }
            }
        }
        assert_eq!(made, MAX_HOLDERS_PER_ROOM);
    }

    #[test]
    fn labels_are_tidied_and_bounded() {
        assert_eq!(clean_holder_label("  \u{202e}Sky\n "), "Sky");
        assert_eq!(clean_holder_label(" "), "Holder");
        assert_eq!(clean_holder_label(&"x".repeat(80)).chars().count(), 24);
    }

    #[test]
    fn the_owner_moves_resizes_renames_and_the_pieces_come_along() {
        let mut room = room();
        let holder = create(&mut room, "ada", SPOT).unwrap();
        drop_at(&mut room, "ada", 5, 300.0, 1000.0);
        let before = snapshot(&room).piece(5).unwrap().clone();
        let update = |x: f32, label: &str| ClientMessage::UpdateHolder {
            holder_id: holder,
            x,
            y: 800.0,
            width: 1200.0,
            height: 800.0,
            label: label.into(),
        };
        assert_eq!(
            room.apply("bo", update(-1500.0, "Mine"), 0).unwrap_err(),
            "Only its owner can change a holder"
        );
        let out = room.apply("ada", update(-1500.0, "Sky"), 0).unwrap();
        let ServerMessage::PiecesChanged {
            pieces, holders, ..
        } = out.event
        else {
            panic!()
        };
        assert_eq!(pieces.len(), 1);
        assert_eq!(pieces[0].x, before.x + (-1500.0 - SPOT.0));
        let holders = holders.unwrap();
        assert_eq!(
            (holders[0].label.as_str(), holders[0].width),
            ("Sky", 1200.0)
        );
        // Still the owner's piece in its new spot.
        assert!(
            room.apply("bo", ClientMessage::DragStart { piece_id: 5 }, 0)
                .is_err()
        );
    }

    #[test]
    fn moving_a_holder_leaves_a_group_that_straddles_another_players_holder() {
        let mut room = room();
        let ada = create(&mut room, "ada", SPOT).unwrap();
        create(&mut room, "bo", (-1200.0, 800.0, 1000.0, 800.0)).unwrap();
        let snap = room.snapshot.as_mut().unwrap();
        // Pieces 0 and 1 joined: 0's centre in Bo's holder, 1's in Ada's.
        for (id, x) in [(0, -400.0), (1, -200.0)] {
            let piece = snap.piece_mut(id).unwrap();
            piece.group_id = 0;
            piece.x = x;
            piece.y = 800.0;
        }
        let before = snapshot(&room).pieces.clone();
        let out = room
            .apply(
                "ada",
                ClientMessage::UpdateHolder {
                    holder_id: ada,
                    x: -200.0,
                    y: 1700.0,
                    width: 1000.0,
                    height: 800.0,
                    label: "Edges".into(),
                },
                0,
            )
            .unwrap();
        let ServerMessage::PiecesChanged { pieces, .. } = out.event else {
            panic!()
        };
        assert!(pieces.is_empty());
        assert_eq!(snapshot(&room).pieces, before);
    }

    #[test]
    fn moving_a_holder_never_drops_a_group_into_another_players_holder() {
        let mut room = room();
        let ada = create(&mut room, "ada", SPOT).unwrap();
        // Bo's holder is where Ada's piece would land if Ada shrinks hers.
        create(&mut room, "bo", (-700.0, 2400.0, 1000.0, 800.0)).unwrap();
        drop_at(&mut room, "ada", 5, 700.0, 1500.0);
        drop_at(&mut room, "ada", 6, -100.0, 900.0);
        let kept = snapshot(&room).piece(5).unwrap().clone();
        let out = room
            .apply(
                "ada",
                ClientMessage::UpdateHolder {
                    holder_id: ada,
                    x: -1400.0,
                    y: 2000.0,
                    width: 400.0,
                    height: 400.0,
                    label: "Edges".into(),
                },
                0,
            )
            .unwrap();
        let ServerMessage::PiecesChanged { pieces, .. } = out.event else {
            panic!()
        };
        // Piece 6 comes along; piece 5 would have landed in Bo's, so stays.
        assert_eq!(pieces.iter().map(|p| p.id).collect::<Vec<_>>(), vec![6]);
        assert_eq!(snapshot(&room).piece(5).unwrap(), &kept);
        let snap = snapshot(&room);
        assert_eq!(
            snap.holder_of(snap.piece(6).unwrap()).map(|h| h.id),
            Some(ada)
        );
    }

    #[test]
    fn deleting_releases_pieces_in_place_and_the_host_may_clear_any() {
        let mut room = room();
        let holder = create(&mut room, "ada", SPOT).unwrap();
        drop_at(&mut room, "ada", 5, 300.0, 1000.0);
        let spot = snapshot(&room).piece(5).unwrap().clone();
        assert!(
            room.apply("bo", ClientMessage::DeleteHolder { holder_id: holder }, 0)
                .is_err()
        );
        room.apply("host", ClientMessage::DeleteHolder { holder_id: holder }, 0)
            .unwrap();
        assert!(snapshot(&room).holders.is_empty());
        assert_eq!(snapshot(&room).piece(5).unwrap(), &spot);
        room.apply("bo", ClientMessage::DragStart { piece_id: 5 }, 0)
            .unwrap();
    }

    #[test]
    fn a_departed_owner_keeps_holders_for_the_grace_period_then_loses_them() {
        let mut room = room();
        create(&mut room, "ada", SPOT).unwrap();
        drop_at(&mut room, "ada", 5, 300.0, 1000.0);
        room.names.remove("ada");
        // Going away is announced, once, so everyone sees "(away)".
        let revision = snapshot(&room).revision;
        let Some(ServerMessage::PiecesChanged {
            holders: Some(holders),
            revision: sent,
            ..
        }) = room.tick_holders(1_000)
        else {
            panic!("going away is announced with the holders")
        };
        assert_eq!(
            (sent, holders[0].owner_away_since_ms),
            (revision + 1, Some(1_000))
        );
        assert!(room.tick_holders(1_500).is_none());
        assert_eq!(snapshot(&room).holders[0].owner_away_since_ms, Some(1_000));
        // Still reserved while Ada is away.
        assert!(
            room.apply("bo", ClientMessage::DragStart { piece_id: 5 }, 0)
                .is_err()
        );
        // Ada comes back under a new connection and reclaims it.
        room.names.insert("ada-2".into(), "Ada".into());
        assert!(
            room.tick_holders(2_000).is_some(),
            "coming back is announced"
        );
        assert_eq!(snapshot(&room).holders[0].owner_away_since_ms, None);
        assert!(room.tick_holders(2_500).is_none());
        room.names.remove("ada-2");
        room.tick_holders(3_000).unwrap();
        assert!(room.tick_holders(3_000 + HOLDER_GRACE_MS - 1).is_none());
        let revision = snapshot(&room).revision;
        let event = room.tick_holders(3_000 + HOLDER_GRACE_MS).unwrap();
        let ServerMessage::PiecesChanged {
            pieces,
            revision: sent,
            holders,
            ..
        } = event
        else {
            panic!()
        };
        assert!(pieces.is_empty());
        assert_eq!(sent, revision + 1);
        assert_eq!(holders, Some(Vec::new()));
        room.apply("bo", ClientMessage::DragStart { piece_id: 5 }, 0)
            .unwrap();
    }

    #[test]
    fn restart_clears_every_holder() {
        let mut room = room();
        create(&mut room, "ada", SPOT).unwrap();
        let out = room.apply("host", ClientMessage::RestartPuzzle, 7).unwrap();
        let ServerMessage::PiecesChanged { holders, .. } = out.event else {
            panic!()
        };
        assert_eq!(holders, Some(Vec::new()));
        assert!(snapshot(&room).holders.is_empty());
    }

    #[test]
    fn arranging_the_tray_leaves_pieces_in_holders_alone_and_fills_no_holder() {
        let mut room = room();
        create(&mut room, "ada", SPOT).unwrap();
        drop_at(&mut room, "ada", 5, 300.0, 1000.0);
        let kept = snapshot(&room).piece(5).unwrap().clone();
        room.apply(
            "bo",
            ClientMessage::RearrangeTray {
                arrangement: TrayArrangement::EdgesFirst,
            },
            1,
        )
        .unwrap();
        let snap = snapshot(&room);
        assert_eq!(snap.piece(5).unwrap(), &kept);
        let in_holders = snap
            .pieces
            .iter()
            .filter(|piece| snap.holder_of(piece).is_some())
            .count();
        assert_eq!(in_holders, 1);
    }

    #[test]
    fn a_free_spot_avoids_the_board_tray_and_other_holders() {
        let mut room = room();
        let snap = snapshot(&room).clone();
        let (w, h) = (1000.0, 800.0);
        let (x, y) = snap.free_holder_spot(w, h, 400.0, 300.0).unwrap();
        assert!(snap.check_holder_rect(x, y, w, h, None).is_ok());
        create(&mut room, "ada", (x, y, w, h)).unwrap();
        let snap = snapshot(&room);
        let (x2, y2) = snap.free_holder_spot(w, h, 400.0, 300.0).unwrap();
        assert!(snap.check_holder_rect(x2, y2, w, h, None).is_ok());
        assert_ne!((x, y), (x2, y2));
        assert!(snap.free_holder_spot(w, h, f32::NAN, 0.0).is_none());
    }

    #[test]
    fn old_snapshots_load_without_holders_and_imports_drop_them() {
        let mut room = room();
        create(&mut room, "ada", SPOT).unwrap();
        let mut json = serde_json::to_value(snapshot(&room)).unwrap();
        json.as_object_mut().unwrap().remove("holders");
        let older: RoomSnapshot = serde_json::from_value(json).unwrap();
        assert!(older.holders.is_empty());
        let mut portable = snapshot(&room).clone();
        portable.media_sha256 = "a".repeat(64);
        portable.queue[0].sha256 = "a".repeat(64);
        let rehosted = portable
            .rehost("new".into(), "CODE".into(), "h".into())
            .unwrap();
        assert!(rehosted.holders.is_empty());
    }
}
