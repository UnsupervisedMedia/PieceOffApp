//! Spectators and the solve timelapse (protocol v13).
//!
//! A spectator is a connection that watches a room: it receives everything a
//! player does but `apply` refuses every change it asks for. Spectators are
//! kept apart from `RoomRuntime::names`, so they never count toward the
//! solve clock, the player limit, holder presence, or the host.
//!
//! The timelapse is a compact log of where pieces came to rest during the
//! current solve, kept by the host and sent only on request. It starts from
//! every piece's position just before the first piece is picked up (or
//! moved by a tray rearrangement), then adds one frame per settling change:
//! a drop, a tray rearrangement, or a holder moved with its pieces. A
//! restart clears it.

use crate::{NAME_TAKEN, NameClaim, Piece, RoomRuntime, RoomSnapshot};
use serde::{Deserialize, Serialize};

/// Most piece positions one timelapse keeps, its starting layout included.
/// Past this, neighbouring frames are merged (later positions win), so the
/// replay still ends exactly where the room is, just in coarser steps.
pub const MAX_TIMELAPSE_POSITIONS: usize = 20_000;

/// Spectators one room admits, apart from its players.
pub const MAX_ROOM_SPECTATORS: usize = 16;

/// Where pieces came to rest during the current solve. Positions are board
/// coordinates rounded to whole source pixels: `[piece id, x, y]`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Timelapse {
    /// Every piece's position when recording began.
    pub start: Vec<[i32; 3]>,
    /// Settling changes in order.
    pub frames: Vec<TimelapseFrame>,
    /// Some neighbouring frames were merged to stay within
    /// `MAX_TIMELAPSE_POSITIONS`.
    pub condensed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelapseFrame {
    /// The room's solve clock when this change landed, in milliseconds, so
    /// time the room stood empty is left out.
    pub at_ms: u64,
    /// `[piece id, x, y]` for each piece the change moved.
    pub moves: Vec<[i32; 3]>,
}

fn rounded(piece: &Piece) -> [i32; 3] {
    // Clamping keeps a damaged position from wrapping; `as` saturates.
    [
        piece.id as i32,
        piece.x.round() as i32,
        piece.y.round() as i32,
    ]
}

impl Timelapse {
    pub fn is_empty(&self) -> bool {
        self.start.is_empty() && self.frames.is_empty()
    }

    /// Positions held, the starting layout included.
    pub fn positions(&self) -> usize {
        self.start.len() + self.frames.iter().map(|f| f.moves.len()).sum::<usize>()
    }

    /// The solve-clock time of the last frame.
    pub fn duration_ms(&self) -> u64 {
        self.frames.last().map_or(0, |frame| frame.at_ms)
    }

    /// Begin from `pieces` unless recording has already begun.
    pub fn ensure_started(&mut self, pieces: &[Piece]) {
        if self.start.is_empty() {
            self.start = pieces.iter().map(rounded).collect();
            self.frames.clear();
        }
    }

    /// Add a frame for `moved` pieces at solve time `at_ms`.
    pub fn record(&mut self, at_ms: u64, moved: &[Piece]) {
        if moved.is_empty() || self.start.is_empty() {
            return;
        }
        self.frames.push(TimelapseFrame {
            at_ms,
            moves: moved.iter().map(rounded).collect(),
        });
        if self.positions() > MAX_TIMELAPSE_POSITIONS {
            self.condense();
        }
    }

    /// Merge neighbouring frames pairwise until the log is comfortably
    /// within budget. A piece moved in both keeps its later position.
    fn condense(&mut self) {
        let target = MAX_TIMELAPSE_POSITIONS * 3 / 4;
        while self.positions() > target && self.frames.len() > 1 {
            let frames = std::mem::take(&mut self.frames);
            let mut frames = frames.into_iter();
            while let Some(mut first) = frames.next() {
                if let Some(second) = frames.next() {
                    first
                        .moves
                        .retain(|[id, ..]| !second.moves.iter().any(|m| m[0] == *id));
                    first.moves.extend(second.moves);
                    first.at_ms = second.at_ms;
                }
                self.frames.push(first);
            }
            self.condensed = true;
        }
    }

    /// Make a log from storage safe to host: bounded, and dropped entirely
    /// when its starting layout does not match the room's pieces.
    pub fn sanitize(&mut self, snapshot: &RoomSnapshot) {
        let ids_match = self.start.len() == snapshot.pieces.len()
            && self
                .start
                .iter()
                .all(|[id, ..]| u32::try_from(*id).is_ok_and(|id| snapshot.piece(id).is_some()));
        if !self.is_empty() && !ids_match {
            *self = Self::default();
            return;
        }
        if self.positions() > MAX_TIMELAPSE_POSITIONS {
            self.condense();
            if self.positions() > MAX_TIMELAPSE_POSITIONS {
                *self = Self::default();
            }
        }
    }
}

/// A room as the host saves it: the snapshot every client sees, plus
/// host-only state beside its fields. Older saves are a bare snapshot and
/// load with none.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredRoom {
    #[serde(flatten)]
    pub snapshot: RoomSnapshot,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub name_claims: Vec<NameClaim>,
    #[serde(default, skip_serializing_if = "Timelapse::is_empty")]
    pub timelapse: Timelapse,
}

impl RoomRuntime {
    /// Whether `client_id` joined to watch.
    pub fn is_spectator(&self, client_id: &str) -> bool {
        self.spectators.contains_key(client_id)
    }

    /// Let `client_id` watch under `name` (already cleaned). Spectators claim
    /// no name, but may not watch under one claimed by another player's key,
    /// so chat cannot be signed with someone else's name.
    pub fn admit_spectator(
        &mut self,
        client_id: &str,
        name: &str,
        key_hash: Option<&str>,
    ) -> Result<(), String> {
        let folded = name.to_lowercase();
        let claimed_by_other = self
            .name_claims
            .iter()
            .any(|claim| claim.name == folded && Some(claim.key_hash.as_str()) != key_hash);
        let own_claim = self
            .name_claims
            .iter()
            .any(|claim| claim.name == folded && Some(claim.key_hash.as_str()) == key_hash);
        // A keyless player holds no claim, so also refuse a name a player in
        // the room is using unless this key owns it.
        let in_use = self
            .names
            .values()
            .any(|seated| seated.to_lowercase() == folded);
        if claimed_by_other || (in_use && !own_claim) {
            return Err(NAME_TAKEN.into());
        }
        if self.spectators.len() >= MAX_ROOM_SPECTATORS {
            return Err(format!(
                "This room already has {MAX_ROOM_SPECTATORS} spectators"
            ));
        }
        self.spectators
            .insert(client_id.to_owned(), name.to_owned());
        Ok(())
    }

    /// Record the room's timelapse from here on if it has not begun yet.
    pub(crate) fn start_timelapse(&mut self) {
        if let Some(snapshot) = self.snapshot.as_ref() {
            self.timelapse.ensure_started(&snapshot.pieces);
        }
    }

    /// Add a settling change to the timelapse.
    pub(crate) fn record_timelapse(&mut self, moved: &[Piece], now_ms: u64) {
        if let Some(snapshot) = self.snapshot.as_ref() {
            let at_ms = snapshot.stats.elapsed_at(now_ms);
            self.timelapse.record(at_ms, moved);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn spectators_cannot_borrow_a_keyless_players_name() {
        let mut room = RoomRuntime::default();
        room.names.insert("p1".into(), "Ada".into());
        assert_eq!(
            room.admit_spectator("s1", "ada", Some("other")),
            Err(NAME_TAKEN.into())
        );
        assert_eq!(
            room.admit_spectator("s1", "Ada", None),
            Err(NAME_TAKEN.into())
        );
        assert!(room.admit_spectator("s2", "Bo", None).is_ok());
    }

    use super::*;
    use crate::{ClientMessage, ServerMessage};
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
            names: HashMap::from([("host".into(), "Ada".into())]),
            spectators: HashMap::from([("watcher".into(), "Cy".into())]),
            host_id: Some("host".into()),
            ..RoomRuntime::default()
        }
    }

    fn drop_piece(room: &mut RoomRuntime, client: &str, piece: u32, x: f32, y: f32, now: u64) {
        room.apply(client, ClientMessage::DragStart { piece_id: piece }, now)
            .unwrap();
        room.apply(
            client,
            ClientMessage::DragEnd {
                piece_id: piece,
                x,
                y,
            },
            now,
        )
        .unwrap();
    }

    #[test]
    fn spectators_cannot_change_anything() {
        let mut room = room();
        let before = room.snapshot.clone();
        let refused = [
            ClientMessage::DragStart { piece_id: 0 },
            ClientMessage::DragStartSelection { piece_ids: vec![0] },
            ClientMessage::MovePiece {
                piece_id: 0,
                x: 1.0,
                y: 1.0,
            },
            ClientMessage::DragEnd {
                piece_id: 0,
                x: 1.0,
                y: 1.0,
            },
            ClientMessage::DragEndSelection {
                anchor_piece_id: 0,
                x: 1.0,
                y: 1.0,
            },
            ClientMessage::SetPlayback {
                playing: true,
                position_ms: 0,
                rate: 1.0,
                loop_start_ms: 0,
                loop_end_ms: 0,
            },
            ClientMessage::RestartPuzzle,
            ClientMessage::RearrangeTray {
                arrangement: crate::TrayArrangement::Shuffle,
            },
            ClientMessage::NextItem,
            ClientMessage::PrevItem,
            ClientMessage::SelectItem { index: 0 },
            ClientMessage::QueueRemove { index: 0 },
            ClientMessage::QueueMove { from: 0, to: 0 },
            ClientMessage::SetRepeat {
                mode: crate::RepeatMode::All,
            },
            ClientMessage::SetVideoLock { locked: true },
            ClientMessage::CreateHolder {
                x: -500.0,
                y: 0.0,
                width: 400.0,
                height: 400.0,
                label: String::new(),
            },
            ClientMessage::DeleteHolder { holder_id: 1 },
            ClientMessage::RequestTimelapse,
        ];
        for message in refused {
            let label = format!("{message:?}");
            assert!(room.apply("watcher", message, 1_000).is_err(), "{label}");
        }
        assert_eq!(room.snapshot, before);
        assert!(room.timelapse.is_empty());
        assert!(!room.snapshot.as_ref().unwrap().stats.started);
    }

    #[test]
    fn a_room_with_only_spectators_pauses_the_clock() {
        let mut room = room();
        room.apply("host", ClientMessage::DragStart { piece_id: 0 }, 1_000)
            .unwrap();
        room.names.clear();
        room.sync_clock_to_players(3_000);
        let stats = &room.snapshot.as_ref().unwrap().stats;
        assert!(!stats.running());
        assert_eq!(stats.elapsed_at(60_000), 2_000);
        assert!(room.is_spectator("watcher"));
        assert!(!room.is_spectator("host"));
    }

    #[test]
    fn drops_are_recorded_from_the_layout_before_the_first_pickup() {
        let mut room = room();
        let scattered: Vec<[i32; 3]> = room
            .snapshot
            .as_ref()
            .unwrap()
            .pieces
            .iter()
            .map(rounded)
            .collect();
        drop_piece(&mut room, "host", 3, 1_000.4, 20.6, 1_000);
        room.apply("host", ClientMessage::DragStart { piece_id: 4 }, 2_000)
            .unwrap();
        // Moves while dragging are not frames.
        room.apply(
            "host",
            ClientMessage::MovePiece {
                piece_id: 4,
                x: 50.0,
                y: 50.0,
            },
            2_500,
        )
        .unwrap();
        room.apply(
            "host",
            ClientMessage::DragEnd {
                piece_id: 4,
                x: 60.0,
                y: 70.0,
            },
            4_000,
        )
        .unwrap();
        let log = &room.timelapse;
        assert_eq!(log.start, scattered);
        assert_eq!(log.frames.len(), 2);
        assert_eq!(log.frames[0].moves, vec![[3, 1_000, 21]]);
        // Solve clock time: it started at the first pickup.
        assert_eq!(log.frames[0].at_ms, 0);
        assert_eq!(log.frames[1].at_ms, 3_000);
        assert_eq!(log.duration_ms(), 3_000);
    }

    #[test]
    fn a_restart_clears_the_log_and_the_next_solve_starts_afresh() {
        let mut room = room();
        drop_piece(&mut room, "host", 0, 900.0, 10.0, 1_000);
        assert!(!room.timelapse.is_empty());
        let out = room
            .apply("host", ClientMessage::RestartPuzzle, 2_000)
            .unwrap();
        assert!(matches!(out.event, ServerMessage::PiecesChanged { .. }));
        assert!(room.timelapse.is_empty());
        let rescattered: Vec<[i32; 3]> = room
            .snapshot
            .as_ref()
            .unwrap()
            .pieces
            .iter()
            .map(rounded)
            .collect();
        drop_piece(&mut room, "host", 1, 900.0, 10.0, 3_000);
        assert_eq!(room.timelapse.start, rescattered);
        assert_eq!(room.timelapse.frames.len(), 1);
    }

    #[test]
    fn rearranging_the_tray_is_recorded() {
        let mut room = room();
        room.apply(
            "host",
            ClientMessage::RearrangeTray {
                arrangement: crate::TrayArrangement::EdgesFirst,
            },
            1_000,
        )
        .unwrap();
        assert_eq!(room.timelapse.start.len(), 12);
        assert_eq!(room.timelapse.frames.len(), 1);
    }

    #[test]
    fn the_log_is_bounded_by_merging_frames_and_ends_where_the_room_is() {
        let pieces: Vec<Piece> = (0..10)
            .map(|id| Piece {
                id,
                group_id: id,
                row: 0,
                column: id as u16,
                x: 0.0,
                y: 0.0,
                placed: false,
                held_by: None,
            })
            .collect();
        let mut log = Timelapse::default();
        log.ensure_started(&pieces);
        let mut last = pieces.clone();
        for step in 0..(MAX_TIMELAPSE_POSITIONS as u64) {
            let mut moved = pieces[(step % 10) as usize].clone();
            moved.x = step as f32;
            last[moved.id as usize] = moved.clone();
            log.record(step * 10, &[moved]);
            assert!(log.positions() <= MAX_TIMELAPSE_POSITIONS);
        }
        assert!(log.condensed);
        assert_eq!(log.duration_ms(), (MAX_TIMELAPSE_POSITIONS as u64 - 1) * 10);
        // Replaying every frame still lands on the final layout.
        let mut at: HashMap<i32, [i32; 3]> = log.start.iter().map(|p| (p[0], *p)).collect();
        for frame in &log.frames {
            for position in &frame.moves {
                at.insert(position[0], *position);
            }
        }
        for piece in &last {
            assert_eq!(at[&(piece.id as i32)], rounded(piece));
        }
    }

    #[test]
    fn stored_rooms_carry_the_timelapse_and_bare_snapshots_still_load() {
        let mut room = room();
        drop_piece(&mut room, "host", 0, 900.0, 10.0, 1_000);
        let stored = StoredRoom {
            snapshot: room.snapshot.clone().unwrap(),
            name_claims: Vec::new(),
            timelapse: room.timelapse.clone(),
        };
        let text = serde_json::to_string(&stored).unwrap();
        let back: StoredRoom = serde_json::from_str(&text).unwrap();
        assert_eq!(back, stored);
        // The stored form is still a snapshot to older readers.
        let snapshot: RoomSnapshot = serde_json::from_str(&text).unwrap();
        assert_eq!(&snapshot, room.snapshot.as_ref().unwrap());
        // A v12 save is a bare snapshot.
        let old = serde_json::to_string(room.snapshot.as_ref().unwrap()).unwrap();
        assert!(!old.contains("timelapse"));
        let back: StoredRoom = serde_json::from_str(&old).unwrap();
        assert!(back.timelapse.is_empty());
        // An empty timelapse is not written at all.
        let empty = StoredRoom {
            snapshot: room.snapshot.clone().unwrap(),
            name_claims: Vec::new(),
            timelapse: Timelapse::default(),
        };
        assert!(!serde_json::to_string(&empty).unwrap().contains("timelapse"));
    }

    #[test]
    fn a_stored_log_that_does_not_fit_the_room_is_dropped() {
        let mut room = room();
        drop_piece(&mut room, "host", 0, 900.0, 10.0, 1_000);
        let snapshot = room.snapshot.clone().unwrap();
        let mut log = room.timelapse.clone();
        log.sanitize(&snapshot);
        assert_eq!(log, room.timelapse);
        log.start.pop();
        log.sanitize(&snapshot);
        assert!(log.is_empty());
    }

    #[test]
    fn spectators_cannot_watch_under_a_name_another_key_claimed_and_are_capped() {
        let mut room = RoomRuntime::default();
        room.admit_player("ada", "Ada", Some("k1"), 1).unwrap();
        assert_eq!(
            room.admit_spectator("w1", "ADA", Some("k2")).unwrap_err(),
            NAME_TAKEN
        );
        assert_eq!(
            room.admit_spectator("w1", "ada", None).unwrap_err(),
            NAME_TAKEN
        );
        // Ada herself may watch from another window.
        room.admit_spectator("w1", "Ada", Some("k1")).unwrap();
        assert_eq!(room.names.len(), 1, "watching seats nobody");
        for index in 1..MAX_ROOM_SPECTATORS {
            room.admit_spectator(&format!("w{}", index + 1), "W", None)
                .unwrap();
        }
        assert!(room.admit_spectator("late", "W", None).is_err());
        assert!(room.name_claims.iter().all(|claim| claim.name != "w"));
    }

    #[test]
    fn join_room_spectator_flag_defaults_to_playing() {
        let text = r#"{"type":"join_room","invite_code":"A","name":"n","media_sha256":"","media_extension":"png"}"#;
        match serde_json::from_str::<ClientMessage>(text).unwrap() {
            ClientMessage::JoinRoom { spectator, .. } => assert!(!spectator),
            other => panic!("unexpected {other:?}"),
        }
        let text = r#"{"type":"peer_joined","name":"n"}"#;
        match serde_json::from_str::<ServerMessage>(text).unwrap() {
            ServerMessage::PeerJoined { spectator, .. } => assert!(!spectator),
            other => panic!("unexpected {other:?}"),
        }
    }
}
