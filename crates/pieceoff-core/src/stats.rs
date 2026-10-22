//! The room's shared solve clock and who assembled what. The host owns both:
//! clients only display them, ticking the clock locally between updates.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::{RoomRuntime, RoomSnapshot, clean_name};

/// Most players one room's stats remember. Later newcomers still count
/// toward the total, just not by name.
pub const MAX_SOLVE_PLAYERS: usize = 32;

/// How long the current run has taken and how many pieces each player
/// brought into the picture. Saved with the room, reset by a restart.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SolveStats {
    /// Solving time banked before `running_since_ms`.
    pub elapsed_ms: u64,
    /// Host clock (ms since the Unix epoch) when the clock last resumed.
    /// `None` while it is stopped: not started, paused because nobody is in
    /// the room, or finished. A saved room is always stopped.
    pub running_since_ms: Option<u64>,
    /// Someone has picked up a piece since the room was made or restarted.
    pub started: bool,
    /// The puzzle was completed; the clock stays stopped until a restart.
    pub finished: bool,
    /// Pieces each player brought into the assembled picture, by display
    /// name, in the order they first scored.
    pub players: Vec<PlayerSolveStats>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerSolveStats {
    pub name: String,
    pub pieces: u32,
}

impl SolveStats {
    /// Solving time at `now_ms` on the host clock.
    pub fn elapsed_at(&self, now_ms: u64) -> u64 {
        let running = self
            .running_since_ms
            .map_or(0, |since| now_ms.saturating_sub(since));
        self.elapsed_ms.saturating_add(running)
    }

    pub fn running(&self) -> bool {
        self.running_since_ms.is_some()
    }

    /// Start the clock for a fresh run. Does nothing once started.
    pub fn start(&mut self, now_ms: u64) {
        if !self.started {
            self.started = true;
            self.finished = false;
            self.running_since_ms = Some(now_ms);
        }
    }

    /// Bank the running time and stop the clock.
    pub fn pause(&mut self, now_ms: u64) {
        if let Some(since) = self.running_since_ms.take() {
            self.elapsed_ms = self.elapsed_ms.saturating_add(now_ms.saturating_sub(since));
        }
    }

    /// Run a paused clock again, unless it never started or has finished.
    pub fn resume(&mut self, now_ms: u64) {
        if self.started && !self.finished && self.running_since_ms.is_none() {
            self.running_since_ms = Some(now_ms);
        }
    }

    /// Stop the clock for good: the puzzle is done.
    pub fn finish(&mut self, now_ms: u64) {
        if self.started && !self.finished {
            self.pause(now_ms);
            self.finished = true;
        }
    }

    /// Add `pieces` to `name`'s count.
    pub fn credit(&mut self, name: &str, pieces: u32) {
        if pieces == 0 {
            return;
        }
        if let Some(player) = self.players.iter_mut().find(|player| player.name == name) {
            player.pieces = player.pieces.saturating_add(pieces);
        } else if self.players.len() < MAX_SOLVE_PLAYERS {
            self.players.push(PlayerSolveStats {
                name: name.to_owned(),
                pieces,
            });
        }
    }

    /// Players by pieces, most first, then by name.
    pub fn ranking(&self) -> Vec<&PlayerSolveStats> {
        let mut players: Vec<_> = self.players.iter().collect();
        players.sort_by(|a, b| b.pieces.cmp(&a.pieces).then_with(|| a.name.cmp(&b.name)));
        players
    }

    /// Make stats from an untrusted snapshot (an imported package) safe to
    /// host: stopped, bounded, one entry per tidy name.
    pub(crate) fn sanitize(&mut self) {
        self.running_since_ms = None;
        if !self.started {
            self.finished = false;
        }
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut players: Vec<PlayerSolveStats> = Vec::new();
        for player in std::mem::take(&mut self.players) {
            let name = clean_name(&player.name);
            match seen.get(&name) {
                Some(&index) => {
                    players[index].pieces = players[index].pieces.saturating_add(player.pieces)
                }
                None if players.len() < MAX_SOLVE_PLAYERS => {
                    seen.insert(name.clone(), players.len());
                    players.push(PlayerSolveStats {
                        name,
                        pieces: player.pieces,
                    });
                }
                None => {}
            }
        }
        self.players = players;
    }
}

impl RoomSnapshot {
    /// Pieces that are part of the assembled picture: snapped to the board
    /// or joined to at least one neighbour.
    pub fn settled_pieces(&self) -> usize {
        let mut group_sizes: HashMap<u32, usize> = HashMap::new();
        for piece in &self.pieces {
            *group_sizes.entry(piece.group_id).or_default() += 1;
        }
        self.pieces
            .iter()
            .filter(|piece| piece.placed || group_sizes[&piece.group_id] > 1)
            .count()
    }

    /// This snapshot as it should be saved at `now_ms`: the running clock
    /// banked and stopped, since a host cannot know how long it will be down.
    pub fn for_storage(&self, now_ms: u64) -> RoomSnapshot {
        let mut stored = self.clone();
        stored.stats.pause(now_ms);
        stored
    }
}

impl RoomRuntime {
    /// Hosts call this whenever players connect or leave: the clock runs only
    /// while someone is in the room.
    pub fn set_occupied(&mut self, occupied: bool, now_ms: u64) {
        if let Some(snapshot) = self.snapshot.as_mut() {
            if occupied {
                snapshot.stats.resume(now_ms);
            } else {
                snapshot.stats.pause(now_ms);
            }
        }
    }

    /// `set_occupied` from the runtime's own list of connected players.
    pub fn sync_clock_to_players(&mut self, now_ms: u64) {
        let occupied = !self.names.is_empty();
        self.set_occupied(occupied, now_ms);
    }

    /// The first piece picked up after the room is made or restarted starts
    /// the clock. A finished room's clock stays stopped.
    pub(crate) fn note_pickup(&mut self, now_ms: u64) {
        if let Some(snapshot) = self.snapshot.as_mut() {
            if !snapshot.completed() {
                snapshot.stats.start(now_ms);
            }
        }
    }

    /// After a drop: credit the pieces it brought into the picture to the
    /// player who dropped them, and stop the clock if that finished it.
    pub(crate) fn note_drop(&mut self, client_id: &str, settled_before: usize, now_ms: u64) {
        let name = self
            .names
            .get(client_id)
            .cloned()
            .unwrap_or_else(|| clean_name(""));
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        let gained = snapshot.settled_pieces().saturating_sub(settled_before);
        snapshot
            .stats
            .credit(&name, u32::try_from(gained).unwrap_or(u32::MAX));
        if snapshot.completed() {
            snapshot.stats.finish(now_ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClientMessage, ServerMessage};

    fn room() -> RoomRuntime {
        let snapshot = RoomSnapshot::create(
            "id".into(),
            "invite".into(),
            "test".into(),
            "hash".into(),
            "png".into(),
            800,
            600,
            2,
            2,
            "host".into(),
        )
        .unwrap();
        RoomRuntime {
            snapshot: Some(snapshot),
            names: HashMap::from([
                ("host".into(), "Ada".into()),
                ("guest".into(), "Bob".into()),
            ]),
            host_id: Some("host".into()),
            ..RoomRuntime::default()
        }
    }

    fn stats(room: &RoomRuntime) -> &SolveStats {
        &room.snapshot.as_ref().unwrap().stats
    }

    /// Pick up `piece` and drop it on its own cell, so it snaps.
    fn place(room: &mut RoomRuntime, client: &str, piece: u32, now: u64) -> ServerMessage {
        room.apply(client, ClientMessage::DragStart { piece_id: piece }, now)
            .unwrap();
        let snapshot = room.snapshot.as_ref().unwrap();
        let target = snapshot.piece(piece).unwrap();
        let (x, y) = (
            target.column as f32 * snapshot.cell_width(),
            target.row as f32 * snapshot.cell_height(),
        );
        room.apply(
            client,
            ClientMessage::DragEnd {
                piece_id: piece,
                x,
                y,
            },
            now,
        )
        .unwrap()
        .event
    }

    #[test]
    fn the_clock_starts_at_the_first_pickup_and_says_so() {
        let mut room = room();
        assert!(!stats(&room).started);
        assert_eq!(stats(&room).elapsed_at(5_000), 0);
        let out = room
            .apply("host", ClientMessage::DragStart { piece_id: 0 }, 1_000)
            .unwrap();
        match out.event {
            ServerMessage::PiecesChanged {
                stats: Some(sent), ..
            } => {
                assert!(sent.started);
                assert_eq!(sent.running_since_ms, Some(1_000));
            }
            other => panic!("expected pieces with stats, got {other:?}"),
        }
        assert_eq!(stats(&room).elapsed_at(4_000), 3_000);
        // Picking up again does not restart it, or resend it.
        room.apply(
            "host",
            ClientMessage::DragEnd {
                piece_id: 0,
                x: 2_000.0,
                y: 0.0,
            },
            2_000,
        )
        .unwrap();
        let again = room
            .apply("host", ClientMessage::DragStart { piece_id: 0 }, 3_000)
            .unwrap();
        assert!(matches!(
            again.event,
            ServerMessage::PiecesChanged { stats: None, .. }
        ));
        assert_eq!(stats(&room).running_since_ms, Some(1_000));
    }

    #[test]
    fn an_empty_room_pauses_the_clock_and_a_player_resumes_it() {
        let mut room = room();
        room.apply("host", ClientMessage::DragStart { piece_id: 0 }, 1_000)
            .unwrap();
        room.set_occupied(false, 4_000);
        assert!(!stats(&room).running());
        assert_eq!(stats(&room).elapsed_at(100_000), 3_000);
        room.set_occupied(true, 100_000);
        assert_eq!(stats(&room).elapsed_at(101_000), 4_000);
        // From the player list.
        room.names.clear();
        room.sync_clock_to_players(102_000);
        assert_eq!(stats(&room).elapsed_at(200_000), 5_000);
    }

    #[test]
    fn a_clock_that_never_started_does_not_resume() {
        let mut room = room();
        room.set_occupied(true, 1_000);
        assert!(!stats(&room).running());
        assert_eq!(stats(&room).elapsed_at(9_000), 0);
    }

    #[test]
    fn completing_stops_the_clock_and_credits_whoever_dropped_the_pieces() {
        let mut room = room();
        place(&mut room, "host", 0, 1_000);
        place(&mut room, "host", 1, 2_000);
        place(&mut room, "guest", 2, 3_000);
        assert!(stats(&room).running());
        let last = place(&mut room, "guest", 3, 11_000);
        let ServerMessage::PiecesChanged {
            stats: Some(sent), ..
        } = last
        else {
            panic!("the finishing drop carries the stats");
        };
        assert!(sent.finished);
        assert!(!sent.running());
        assert_eq!(sent.elapsed_at(99_000), 10_000);
        let ranking: Vec<_> = sent
            .ranking()
            .iter()
            .map(|player| (player.name.as_str(), player.pieces))
            .collect();
        assert_eq!(ranking, vec![("Ada", 2), ("Bob", 2)]);
        // Every piece is credited exactly once.
        assert_eq!(sent.players.iter().map(|p| p.pieces).sum::<u32>(), 4);
        // A finished clock does not resume when someone comes back.
        room.set_occupied(true, 50_000);
        assert_eq!(stats(&room).elapsed_at(99_000), 10_000);
    }

    #[test]
    fn joining_two_loose_pieces_credits_both() {
        let mut room = room();
        // Drop piece 1 just right of piece 0, wherever piece 0 lies.
        let anchor = room.snapshot.as_ref().unwrap().piece(0).unwrap().clone();
        let width = room.snapshot.as_ref().unwrap().cell_width();
        room.apply("guest", ClientMessage::DragStart { piece_id: 1 }, 0)
            .unwrap();
        room.apply(
            "guest",
            ClientMessage::DragEnd {
                piece_id: 1,
                x: anchor.x + width,
                y: anchor.y,
            },
            0,
        )
        .unwrap();
        assert_eq!(
            stats(&room).players,
            vec![PlayerSolveStats {
                name: "Bob".into(),
                pieces: 2
            }]
        );
    }

    #[test]
    fn restart_resets_the_clock_and_the_credits() {
        let mut room = room();
        for piece in 0..4 {
            place(&mut room, "host", piece, 1_000 + u64::from(piece));
        }
        assert!(stats(&room).finished);
        let out = room
            .apply("host", ClientMessage::RestartPuzzle, 20_000)
            .unwrap();
        match out.event {
            ServerMessage::PiecesChanged {
                stats: Some(sent), ..
            } => assert_eq!(sent, SolveStats::default()),
            other => panic!("restart must send fresh stats, got {other:?}"),
        }
        room.apply("host", ClientMessage::DragStart { piece_id: 0 }, 30_000)
            .unwrap();
        assert_eq!(stats(&room).elapsed_at(31_000), 1_000);
    }

    #[test]
    fn credits_are_bounded_by_player() {
        let mut stats = SolveStats::default();
        for index in 0..(MAX_SOLVE_PLAYERS + 5) {
            stats.credit(&format!("P{index}"), 1);
        }
        stats.credit("P0", 2);
        assert_eq!(stats.players.len(), MAX_SOLVE_PLAYERS);
        assert_eq!(stats.players[0].pieces, 3);
        stats.credit("P1", 0);
        assert_eq!(stats.players[1].pieces, 1);
    }

    #[test]
    fn stored_snapshots_bank_the_clock_and_older_ones_load_as_zero() {
        let mut room = room();
        room.apply("host", ClientMessage::DragStart { piece_id: 0 }, 1_000)
            .unwrap();
        let stored = room.snapshot.as_ref().unwrap().for_storage(6_000);
        assert!(!stored.stats.running());
        assert_eq!(stored.stats.elapsed_ms, 5_000);
        // The live room keeps running.
        assert!(stats(&room).running());

        let mut json = serde_json::to_value(&stored).unwrap();
        json.as_object_mut().unwrap().remove("stats");
        let older: RoomSnapshot = serde_json::from_value(json).unwrap();
        assert_eq!(older.stats, SolveStats::default());
    }

    #[test]
    fn a_loaded_snapshot_is_stopped_until_a_player_is_present() {
        let mut room = room();
        room.apply("host", ClientMessage::DragStart { piece_id: 0 }, 1_000)
            .unwrap();
        // A snapshot saved mid-run without banking (as an older host did).
        let snapshot = room.snapshot.clone().unwrap();
        let mut loaded = RoomRuntime::from_snapshot(snapshot);
        assert!(!stats(&loaded).running());
        loaded.set_occupied(true, 50_000);
        assert_eq!(stats(&loaded).elapsed_at(52_000), 2_000);
    }

    #[test]
    fn imported_stats_are_stopped_tidied_and_bounded() {
        let mut stats = SolveStats {
            elapsed_ms: 7,
            running_since_ms: Some(1),
            started: true,
            finished: true,
            players: (0..40)
                .map(|index| PlayerSolveStats {
                    name: format!("  P{}\u{7}", index % 35),
                    pieces: 1,
                })
                .collect(),
        };
        stats.sanitize();
        assert!(!stats.running());
        assert_eq!(stats.players.len(), MAX_SOLVE_PLAYERS);
        assert_eq!(stats.players[0].name, "P0");
        assert_eq!(stats.players[0].pieces, 2);
        assert_eq!(stats.elapsed_ms, 7);
    }
}
