//! Name claims: who may play under a display name in a room.
//!
//! Holders, solve credits and colours key by display name, so a name must not
//! be borrowable. Each device keeps a private random player key and sends it
//! when it creates or joins a room. The first keyed player to use a name in a
//! room claims it; the host remembers a hash of the key (never the key) and
//! refuses that name to anyone else. A claim lapses once nobody has played
//! under the name for `HOLDER_GRACE_MS` (the same grace a player's holders
//! get), and the host may release an absent player's name sooner, so a player
//! who lost their settings or moved to another computer can come back.
//! Claims are host-only state: they live in
//! `RoomRuntime`, never in `RoomSnapshot`, so no client message or `.pieceoff`
//! package carries them.

use crate::{HOLDER_GRACE_MS, RoomRuntime};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Names a room remembers claims for. The oldest claim whose player is not
/// in the room is forgotten to make space.
pub const MAX_NAME_CLAIMS: usize = 64;
/// Players a room seats at a time.
pub const MAX_ROOM_PLAYERS: usize = 8;
/// Longest player key a host accepts, in bytes.
pub const MAX_PLAYER_KEY_BYTES: usize = 128;
pub const NAME_TAKEN: &str = "That name is taken in this room. Pick another name, or ask the host to release it (names free up 10 minutes after their player leaves).";
/// Sent to a connection just before it is closed because the same player
/// joined again elsewhere. Clients do not reconnect after it.
pub const REPLACED_BY_NEW_CONNECTION: &str = "This name was opened in another window or on another device using the same settings, so this window was disconnected.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameClaim {
    /// The display name, lowercased: claims ignore case.
    pub name: String,
    /// Hex digest of the claiming player's key, made by the host.
    pub key_hash: String,
    /// Host clock (ms since the Unix epoch) of the latest join under it.
    pub last_seen_ms: u64,
    /// Host clock when the host first saw nobody in the room under this
    /// name; cleared when the name is back. The claim lapses
    /// `HOLDER_GRACE_MS` later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub away_since_ms: Option<u64>,
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
                    away_since_ms: None,
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

    /// Keep claims in step with who is in the room, like `tick_holders`:
    /// stamp a claim when nobody is playing under its name, clear the stamp
    /// when someone is, and drop it `HOLDER_GRACE_MS` after the stamp. The
    /// lapsed name's holders are expired with it, so the next `tick_holders`
    /// removes them and nobody inherits them under the freed name. Hosts call
    /// this on their timer, before `tick_holders`. Returns whether any claim
    /// changed, so the host should save the room.
    pub fn tick_claims(&mut self, now_ms: u64) -> bool {
        let present: HashSet<String> = self.names.values().map(|n| n.to_lowercase()).collect();
        let mut changed = false;
        let mut lapsed = Vec::new();
        for claim in &mut self.name_claims {
            if present.contains(&claim.name) {
                changed |= claim.away_since_ms.take().is_some();
            } else {
                match claim.away_since_ms {
                    None => {
                        claim.away_since_ms = Some(now_ms);
                        changed = true;
                    }
                    Some(since) if now_ms.saturating_sub(since) >= HOLDER_GRACE_MS => {
                        lapsed.push(claim.name.clone());
                    }
                    Some(_) => {}
                }
            }
        }
        if lapsed.is_empty() {
            return changed;
        }
        self.name_claims
            .retain(|claim| !lapsed.contains(&claim.name));
        if let Some(snapshot) = self.snapshot.as_mut() {
            for holder in &mut snapshot.holders {
                if lapsed.contains(&holder.owner.to_lowercase()) {
                    holder.owner_away_since_ms = Some(now_ms.saturating_sub(HOLDER_GRACE_MS));
                }
            }
        }
        true
    }

    /// The host frees the name of a player who is not in the room: its
    /// claim goes and its holders are removed at once (pieces stay put), as
    /// if the grace had run out. Returns the new revision.
    pub fn release_name(&mut self, client_id: &str, name: &str) -> Result<u64, String> {
        let snapshot = self.snapshot.as_mut().ok_or("Room is not initialized")?;
        if snapshot.host_id != client_id {
            return Err("Only the host can release a name".into());
        }
        let name = name.trim();
        let folded = name.to_lowercase();
        if self.names.values().any(|n| n.to_lowercase() == folded) {
            return Err(format!(
                "{name} is in the room; only an absent player's name can be released"
            ));
        }
        let claims = self.name_claims.len();
        self.name_claims.retain(|claim| claim.name != folded);
        let holders = snapshot.holders.len();
        snapshot
            .holders
            .retain(|holder| holder.owner.to_lowercase() != folded);
        if claims == self.name_claims.len() && holders == snapshot.holders.len() {
            return Err(format!("Nothing is held under the name {name}"));
        }
        snapshot.revision += 1;
        Ok(snapshot.revision)
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
    fn a_claim_lapses_after_the_grace_and_not_before() {
        let mut room = RoomRuntime::default();
        room.admit_player("a", "Ada", Some("k1"), 0).unwrap();
        assert!(!room.tick_claims(500), "nothing changes while she plays");
        room.names.clear();
        assert!(room.tick_claims(1_000), "going away is stamped (and saved)");
        assert!(!room.tick_claims(1_000 + HOLDER_GRACE_MS - 1));
        assert!(room.admit_player("b", "Ada", Some("k2"), 2_000).is_err());
        assert!(room.admit_player("b", "ada", None, 2_000).is_err());
        assert!(room.tick_claims(1_000 + HOLDER_GRACE_MS));
        assert!(room.name_claims.is_empty());
        // Free again: a keyless (older) client may use it, claiming nothing.
        room.admit_player("c", "ADA", None, 3_000).unwrap();
        assert!(room.name_claims.is_empty());
        room.names.clear();
        // And a new device may claim it.
        room.admit_player("d", "Ada", Some("k2"), 4_000).unwrap();
        assert_eq!(room.name_claims[0].key_hash, "k2");
    }

    #[test]
    fn coming_back_within_the_grace_keeps_the_claim() {
        let mut room = RoomRuntime::default();
        room.admit_player("a", "Ada", Some("k1"), 0).unwrap();
        room.names.clear();
        room.tick_claims(1_000);
        room.admit_player("a2", "Ada", Some("k1"), 2_000).unwrap();
        assert!(room.tick_claims(2_000), "coming back clears the stamp");
        assert_eq!(room.name_claims[0].away_since_ms, None);
        // The first absence no longer counts.
        assert!(!room.tick_claims(1_000 + HOLDER_GRACE_MS));
        room.names.clear();
        room.tick_claims(5_000);
        assert!(!room.tick_claims(5_000 + HOLDER_GRACE_MS - 1));
        assert_eq!(room.name_claims.len(), 1);
        assert!(room.admit_player("b", "Ada", Some("k2"), 6_000).is_err());
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
