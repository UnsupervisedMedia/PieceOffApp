//! Name claims: who may play under a display name in a room.
//!
//! Holders, solve credits and colours key by display name, so a name must not
//! be borrowable. Each device keeps a private random player key and sends it
//! when it creates or joins a room. The first keyed player to use a name in a
//! room claims it; the host remembers a hash of the key (never the key) and
//! refuses that name to anyone else. Claims are host-only state: they live in
//! `RoomRuntime`, never in `RoomSnapshot`, so no client message or `.pieceoff`
//! package carries them.

use crate::RoomRuntime;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Names a room remembers claims for. The oldest claim whose player is not
/// in the room is forgotten to make space.
pub const MAX_NAME_CLAIMS: usize = 64;
/// Players a room seats at a time.
pub const MAX_ROOM_PLAYERS: usize = 8;
/// Longest player key a host accepts, in bytes.
pub const MAX_PLAYER_KEY_BYTES: usize = 128;
pub const NAME_TAKEN: &str = "That name is taken in this room";
/// Sent to a connection just before it is closed because the same player
/// joined again elsewhere. Clients do not reconnect after it.
pub const REPLACED_BY_NEW_CONNECTION: &str =
    "You joined this room again from another window, so this one was disconnected.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameClaim {
    /// The display name, lowercased: claims ignore case.
    pub name: String,
    /// Hex digest of the claiming player's key, made by the host.
    pub key_hash: String,
    /// Host clock (ms since the Unix epoch) of the latest join under it.
    pub last_seen_ms: u64,
}

impl RoomRuntime {
    /// Seat `client_id` under `name` (already cleaned). `key_hash` is the
    /// digest of the player's key, `None` for clients that send none.
    ///
    /// A name claimed by another key, or in use by a player in the room under
    /// another key or no key, is refused. A keyed player rejoining under a
    /// name they claimed replaces their earlier connections: those client
    /// ids are unseated and returned so the host can close them. A keyed
    /// player claims an unclaimed name; a keyless one claims nothing.
    pub fn admit_player(
        &mut self,
        client_id: &str,
        name: &str,
        key_hash: Option<&str>,
        now_ms: u64,
    ) -> Result<Vec<String>, String> {
        let folded = name.to_lowercase();
        let live: Vec<String> = self
            .names
            .iter()
            .filter(|(id, seated)| id.as_str() != client_id && seated.to_lowercase() == folded)
            .map(|(id, _)| id.clone())
            .collect();
        let claim = self.name_claims.iter().position(|c| c.name == folded);
        let replaced = match (claim, key_hash) {
            (Some(index), Some(key)) if self.name_claims[index].key_hash == key => live,
            (Some(_), _) => return Err(NAME_TAKEN.into()),
            (None, _) if !live.is_empty() => return Err(NAME_TAKEN.into()),
            (None, _) => Vec::new(),
        };
        if self.names.len() - replaced.len() >= MAX_ROOM_PLAYERS {
            return Err(format!("This room already has {MAX_ROOM_PLAYERS} players"));
        }
        match (claim, key_hash) {
            (Some(index), _) => self.name_claims[index].last_seen_ms = now_ms,
            (None, Some(key)) => {
                if self.name_claims.len() >= MAX_NAME_CLAIMS {
                    let seated: HashSet<String> =
                        self.names.values().map(|n| n.to_lowercase()).collect();
                    let oldest = self
                        .name_claims
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| !seated.contains(&c.name))
                        .min_by_key(|(_, c)| c.last_seen_ms)
                        .map(|(index, _)| index)
                        .ok_or("This room has too many named players")?;
                    self.name_claims.remove(oldest);
                }
                self.name_claims.push(NameClaim {
                    name: folded,
                    key_hash: key.to_owned(),
                    last_seen_ms: now_ms,
                });
            }
            (None, None) => {}
        }
        for id in &replaced {
            self.names.remove(id);
            self.forget_drag(id);
        }
        self.names.insert(client_id.to_owned(), name.to_owned());
        Ok(replaced)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claimed_name_is_refused_to_other_keys_and_keyless_players() {
        let mut room = RoomRuntime::default();
        assert!(
            room.admit_player("a", "Ada", Some("k1"), 1)
                .unwrap()
                .is_empty()
        );
        room.names.clear();
        // Gone from the room, the name stays hers, whatever the case.
        for (key, name) in [(Some("k2"), "Ada"), (Some("k2"), "ADA"), (None, "ada")] {
            assert_eq!(
                room.admit_player("b", name, key, 2).unwrap_err(),
                NAME_TAKEN
            );
        }
        assert!(room.names.is_empty());
        assert!(
            room.admit_player("c", "Ada", Some("k1"), 3)
                .unwrap()
                .is_empty()
        );
        assert_eq!(room.name_claims[0].last_seen_ms, 3);
    }

    #[test]
    fn the_same_key_replaces_its_earlier_connection() {
        let mut room = RoomRuntime::default();
        room.admit_player("old", "Ada", Some("k1"), 1).unwrap();
        room.admit_player("bo", "Bo", Some("k2"), 1).unwrap();
        assert_eq!(
            room.admit_player("new", "Ada", Some("k1"), 2).unwrap(),
            vec!["old".to_string()]
        );
        assert!(!room.names.contains_key("old"));
        assert_eq!(room.names["new"], "Ada");
        // One device may play two names, each its own.
        room.admit_player("alt", "Cy", Some("k1"), 3).unwrap();
        assert_eq!(room.names.len(), 3);
    }

    #[test]
    fn keyless_players_use_free_names_without_claiming_them() {
        let mut room = RoomRuntime::default();
        room.admit_player("old", "Guest", None, 1).unwrap();
        assert!(room.name_claims.is_empty());
        // In use, so nobody else may take it, keyed or not.
        assert!(room.admit_player("x", "guest", None, 2).is_err());
        assert!(room.admit_player("y", "Guest", Some("k"), 2).is_err());
        room.names.clear();
        // Once they have gone a keyed player may claim it.
        room.admit_player("z", "Guest", Some("k"), 3).unwrap();
        room.names.clear();
        assert!(room.admit_player("w", "Guest", None, 4).is_err());
    }

    #[test]
    fn a_full_room_counts_the_connection_being_replaced() {
        let mut room = RoomRuntime::default();
        for index in 0..MAX_ROOM_PLAYERS {
            room.admit_player(
                &format!("c{index}"),
                &format!("P{index}"),
                Some(&format!("k{index}")),
                1,
            )
            .unwrap();
        }
        assert!(
            room.admit_player("late", "Late", Some("kl"), 2)
                .unwrap_err()
                .contains("8 players")
        );
        assert_eq!(
            room.admit_player("again", "P0", Some("k0"), 2).unwrap(),
            vec!["c0".to_string()]
        );
    }

    #[test]
    fn claims_are_bounded_by_forgetting_the_oldest_absent_player() {
        let mut room = RoomRuntime::default();
        room.admit_player("seated", "Seated", Some("ks"), 0)
            .unwrap();
        for index in 1..MAX_NAME_CLAIMS as u64 {
            room.admit_player("p", &format!("P{index}"), Some("k"), index)
                .unwrap();
            room.names.remove("p");
        }
        assert_eq!(room.name_claims.len(), MAX_NAME_CLAIMS);
        room.admit_player("new", "New", Some("kn"), 1_000).unwrap();
        assert_eq!(room.name_claims.len(), MAX_NAME_CLAIMS);
        // The seated player's older claim stays; P1, the oldest absent, goes.
        assert!(room.name_claims.iter().any(|c| c.name == "seated"));
        assert!(!room.name_claims.iter().any(|c| c.name == "p1"));
        assert!(room.name_claims.iter().any(|c| c.name == "p2"));
    }
}
