//! Minimum moderation for rooms strangers can find (protocol v15).
//!
//! - The host removes a player with `kick_player { name }`: every connection
//!   under that name (players and spectators) leaves at once, their leases,
//!   name claim and holders go, and the room bans the name and the player's
//!   key for the rest of its life. Bans are host state saved with the room
//!   (`StoredRoom.bans`), never sent to clients.
//! - Chat can be switched off by the host (`set_chat_enabled`). A room that
//!   becomes listed starts with chat off; pings still work.
//! - Display names joining a listed room are checked against a short built-in
//!   blocklist (`public_name_blocklist.txt`).
//!
//! Clients name players by display name (that is all `welcome` and
//! `peer_joined` tell them), so kicks are by name, like `release_name`.

use crate::{RoomRuntime, RoomSnapshot};
use serde::{Deserialize, Serialize};

/// Bans a room remembers; the oldest is forgotten to make room.
pub const MAX_ROOM_BANS: usize = 64;
/// Player keys remembered per ban (the claim's and each removed connection's).
const MAX_KEYS_PER_BAN: usize = 4;
/// Sent to a removed connection, and to a removed player who tries to come
/// back. Clients do not reconnect after it.
pub const REMOVED_BY_HOST: &str = "The host removed you from this room.";
/// The refusal for a blocked display name in a public room.
pub const PUBLIC_NAME_REFUSED: &str = "Pick a different name for public rooms";
/// The refusal for chat while the host has it switched off.
pub const CHAT_OFF: &str = "The host turned chat off in this room.";

const BLOCKLIST: &str = include_str!("public_name_blocklist.txt");

/// One removed player: nobody may join this room again under `name`, or with
/// any of `key_hashes` under any name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomBan {
    /// Lowercased display name.
    pub name: String,
    /// Salted digests of the removed player's keys (see `NameClaim`); empty
    /// for a player whose app sent no key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_hashes: Vec<String>,
    /// Host clock when the ban was made.
    pub at_ms: u64,
}

/// What `kick_player` changed. Hosts send, in order: `kicked` to each
/// removed connection (then close it), `peer_left` for each, then
/// `snapshot` if present.
#[derive(Debug)]
pub struct KickOutcome {
    /// The removed player's display name.
    pub name: String,
    /// Connections removed, as `(client id, spectator)`.
    pub removed: Vec<(String, bool)>,
    /// The room after leases and holders were released, when they changed.
    pub snapshot: Option<RoomSnapshot>,
}

fn fold_lookalikes(name: &str) -> String {
    name.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            '0' => 'o',
            '1' | '!' => 'i',
            '3' => 'e',
            '4' | '@' => 'a',
            '5' | '$' => 's',
            '7' => 't',
            other => other,
        })
        .collect()
}

/// Whether `name` may be used in a public room or shown in the public lobby.
/// Invite-only rooms do not ask.
pub fn public_name_allowed(name: &str) -> bool {
    let folded = fold_lookalikes(name);
    let words: Vec<&str> = folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let squashed: String = folded.chars().filter(|c| c.is_alphanumeric()).collect();
    !BLOCKLIST
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .any(|entry| match entry.strip_prefix('*') {
            Some(anywhere) => squashed.contains(anywhere),
            None => words
                .iter()
                .any(|word| *word == entry || word.strip_suffix('s') == Some(entry)),
        })
}

impl RoomSnapshot {
    /// List or unlist, keeping chat off in a room that has just become
    /// public (the host may turn it back on).
    pub(crate) fn note_listing_change(&mut self, was_listed: bool) {
        if self.listed && !was_listed {
            self.chat_enabled = false;
        }
    }
}

impl RoomRuntime {
    /// Whether a name or key was banned from this room.
    pub fn is_banned(&self, name: &str, key_hash: Option<&str>) -> bool {
        let folded = name.trim().to_lowercase();
        self.bans.iter().any(|ban| {
            ban.name == folded
                || key_hash.is_some_and(|key| ban.key_hashes.iter().any(|k| k == key))
        })
    }

    /// The checks every joiner passes before name claims: not banned, and in
    /// a listed room, a name the public may see. Spectators too.
    pub(crate) fn check_admission(&self, name: &str, key_hash: Option<&str>) -> Result<(), String> {
        if self.is_banned(name, key_hash) {
            return Err(REMOVED_BY_HOST.into());
        }
        let listed = self.snapshot.as_ref().is_some_and(|s| s.listed);
        if listed && !public_name_allowed(name) {
            return Err(PUBLIC_NAME_REFUSED.into());
        }
        Ok(())
    }

    /// Remember which key each connection joined with, so a kick can ban the
    /// key of a spectator or a player who claimed no name.
    pub(crate) fn note_member_key(&mut self, client_id: &str, key_hash: Option<&str>) {
        let (names, spectators) = (&self.names, &self.spectators);
        self.member_keys
            .retain(|id, _| names.contains_key(id) || spectators.contains_key(id));
        if let Some(key) = key_hash {
            self.member_keys
                .insert(client_id.to_owned(), key.to_owned());
        }
    }

    /// Host-only: switch room chat on or off. Pings are unaffected.
    pub fn set_chat_enabled(
        &mut self,
        client_id: &str,
        enabled: bool,
    ) -> Result<RoomSnapshot, String> {
        let snapshot = self.snapshot.as_mut().ok_or("Room is not initialized")?;
        if snapshot.host_id != client_id {
            return Err("Only the host can switch chat on or off".into());
        }
        if snapshot.chat_enabled != enabled {
            snapshot.chat_enabled = enabled;
            snapshot.revision += 1;
        }
        Ok(snapshot.clone())
    }

    /// Host-only: remove everyone in the room under `name` (players and
    /// spectators) and ban that name and their keys for the room's life.
    /// Their piece leases, name claim and holders are released at once. A
    /// player who just left can be banned too, while their claim remains.
    pub fn kick_player(
        &mut self,
        client_id: &str,
        name: &str,
        now_ms: u64,
    ) -> Result<KickOutcome, String> {
        let snapshot = self.snapshot.as_ref().ok_or("Room is not initialized")?;
        if self.is_spectator(client_id) || snapshot.host_id != client_id {
            return Err("Only the host can remove a player".into());
        }
        let name = name.trim();
        let folded = name.to_lowercase();
        if folded.is_empty() {
            return Err("Choose a player to remove".into());
        }
        if self
            .names
            .get(client_id)
            .is_some_and(|own| own.to_lowercase() == folded)
        {
            return Err("You can't remove yourself".into());
        }
        let under = |roster: &std::collections::HashMap<String, String>| -> Vec<(String, String)> {
            roster
                .iter()
                .filter(|(_, seated)| seated.to_lowercase() == folded)
                .map(|(id, seated)| (id.clone(), seated.clone()))
                .collect()
        };
        let players = under(&self.names);
        let watchers = under(&self.spectators);
        let claim = self.name_claims.iter().find(|c| c.name == folded).cloned();
        if players.is_empty() && watchers.is_empty() && claim.is_none() {
            return Err(format!("Nobody called {name} is in this room"));
        }
        let display = players
            .first()
            .or(watchers.first())
            .map_or_else(|| name.to_owned(), |(_, seated)| seated.clone());

        let mut key_hashes: Vec<String> = claim.map(|c| c.key_hash).into_iter().collect();
        for (id, _) in players.iter().chain(&watchers) {
            if let Some(key) = self.member_keys.remove(id)
                && !key_hashes.contains(&key)
            {
                key_hashes.push(key);
            }
        }
        key_hashes.truncate(MAX_KEYS_PER_BAN);
        self.bans.retain(|ban| ban.name != folded);
        if self.bans.len() >= MAX_ROOM_BANS {
            self.bans.remove(0);
        }
        self.bans.push(RoomBan {
            name: folded.clone(),
            key_hashes,
            at_ms: now_ms,
        });

        let mut removed = Vec::new();
        for (id, _) in &players {
            self.names.remove(id);
            self.forget_drag(id);
            removed.push((id.clone(), false));
        }
        for (id, _) in &watchers {
            self.spectators.remove(id);
            removed.push((id.clone(), true));
        }
        self.name_claims.retain(|c| c.name != folded);
        let snapshot = self.snapshot.as_mut().expect("checked above");
        let mut changed = false;
        for piece in &mut snapshot.pieces {
            if piece
                .held_by
                .as_ref()
                .is_some_and(|holder| players.iter().any(|(id, _)| id == holder))
            {
                piece.held_by = None;
                changed = true;
            }
        }
        let holders = snapshot.holders.len();
        snapshot
            .holders
            .retain(|holder| holder.owner.to_lowercase() != folded);
        changed |= holders != snapshot.holders.len();
        if changed {
            snapshot.revision += 1;
        }
        Ok(KickOutcome {
            name: display,
            removed,
            snapshot: changed.then(|| snapshot.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BUILTIN_MEDIA_SHA256, ClientMessage, NAME_TAKEN, ServerMessage, StoredRoom};

    fn create(name: &str, listed: bool) -> ClientMessage {
        ClientMessage::CreateRoom {
            name: name.into(),
            title: "Puzzle".into(),
            media_sha256: BUILTIN_MEDIA_SHA256[0].0.into(),
            media_extension: "webp".into(),
            width: 640,
            height: 360,
            rows: 3,
            columns: 4,
            initial_snapshot: None,
            duration_ms: 5000,
            player_key: String::new(),
            cut_style: Default::default(),
            listed,
        }
    }

    fn room(listed: bool) -> RoomRuntime {
        let (mut room, name) =
            RoomRuntime::new_room("host", create("Host", listed), "id".into(), "CODE".into())
                .unwrap();
        room.admit_player("host", &name, Some("host-key"), 1)
            .unwrap();
        room
    }

    fn snapshot_of(event: ServerMessage) -> RoomSnapshot {
        match event {
            ServerMessage::Snapshot { snapshot } => snapshot,
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    #[test]
    fn the_blocklist_catches_slurs_and_sexual_names_but_not_ordinary_words() {
        for bad in [
            "nigger",
            "N1GG3R",
            "n.i.g.g.e.r",
            "xXfaggotXx",
            "big cock",
            "Cocks",
            "pornstar",
            "p0rn",
            "sexy girl",
            "rape",
            "Hitler fan",
            "f*u*c*k",
        ] {
            assert!(!public_name_allowed(bad), "{bad} should be refused");
        }
        for fine in [
            "Ada",
            "Scunthorpe",
            "Peacock",
            "cocktail",
            "Therapist",
            "grapes",
            "Essex",
            "Sussex Sam",
            "Analyst",
            "cucumber",
            "Shiitake",
            "Torpedo",
            "Niger Delta",
            "Guest",
            "José",
        ] {
            assert!(public_name_allowed(fine), "{fine} should be allowed");
        }
    }

    #[test]
    fn listed_rooms_refuse_blocked_names_and_invite_rooms_do_not() {
        let refused = RoomRuntime::new_room("h", create("porn king", true), "i".into(), "C".into());
        assert_eq!(refused.err().as_deref(), Some(PUBLIC_NAME_REFUSED));
        let mut public = room(true);
        assert_eq!(
            public.admit_player("p", "sexy", Some("k"), 2).unwrap_err(),
            PUBLIC_NAME_REFUSED
        );
        assert_eq!(
            public.admit_spectator("w", "sexy", None).unwrap_err(),
            PUBLIC_NAME_REFUSED
        );
        public.admit_player("p", "Pat", Some("k"), 2).unwrap();
        let mut private = room(false);
        private.admit_player("p", "sexy", Some("k"), 2).unwrap();
    }

    #[test]
    fn a_host_with_a_blocked_name_cannot_list_the_room() {
        let (mut room, _) =
            RoomRuntime::new_room("host", create("big cock", false), "id".into(), "C".into())
                .unwrap();
        assert_eq!(
            room.apply("host", ClientMessage::SetListed { listed: true }, 0)
                .unwrap_err(),
            PUBLIC_NAME_REFUSED
        );
    }

    #[test]
    fn listing_turns_chat_off_and_the_host_can_turn_it_back_on() {
        let mut room = room(false);
        assert!(room.snapshot.as_ref().unwrap().chat_enabled);
        let listed = snapshot_of(
            room.apply("host", ClientMessage::SetListed { listed: true }, 0)
                .unwrap()
                .event,
        );
        assert!(listed.listed && !listed.chat_enabled);
        assert!(!self::room(true).snapshot.unwrap().chat_enabled);
        room.names.insert("guest".into(), "Guest".into());
        assert!(
            room.apply("guest", ClientMessage::SetChatEnabled { enabled: true }, 0)
                .is_err()
        );
        let chat = |room: &RoomRuntime, id: &str| {
            room.relay(id, ClientMessage::Chat { text: "hi".into() }, 1, 0)
        };
        assert_eq!(chat(&room, "guest").unwrap_err(), CHAT_OFF);
        // Pings still work while chat is off.
        assert!(
            room.relay("guest", ClientMessage::Ping { x: 1.0, y: 1.0 }, 0, 0)
                .unwrap()
                .is_some()
        );
        let outcome = room
            .apply("host", ClientMessage::SetChatEnabled { enabled: true }, 0)
            .unwrap();
        assert!(outcome.persist);
        assert!(snapshot_of(outcome.event).chat_enabled);
        assert!(chat(&room, "guest").unwrap().is_some());
        // Re-listing a listed room changes nothing.
        let again = snapshot_of(
            room.apply("host", ClientMessage::SetListed { listed: true }, 0)
                .unwrap()
                .event,
        );
        assert!(again.chat_enabled);
    }

    #[test]
    fn older_snapshots_load_with_chat_on() {
        let snapshot = room(false).snapshot.unwrap();
        let mut json: serde_json::Value = serde_json::to_value(&snapshot).unwrap();
        json.as_object_mut().unwrap().remove("chat_enabled");
        let back: RoomSnapshot = serde_json::from_value(json).unwrap();
        assert!(back.chat_enabled);
    }

    #[test]
    fn the_host_removes_a_player_who_cannot_come_back_by_name_or_key() {
        let mut room = room(true);
        room.admit_player("p1", "Troll", Some("troll-key"), 2)
            .unwrap();
        room.admit_spectator("w1", "troll", Some("other-key"))
            .unwrap_err();
        room.admit_spectator("w2", "Lurker", Some("lurk-key"))
            .unwrap();
        room.apply("p1", ClientMessage::DragStart { piece_id: 0 }, 3)
            .unwrap();
        room.apply(
            "p1",
            ClientMessage::CreateHolder {
                x: -200.0,
                y: -200.0,
                width: 150.0,
                height: 150.0,
                label: String::new(),
            },
            3,
        )
        .ok();
        let revision = room.snapshot.as_ref().unwrap().revision;
        // Only the host kicks, and never themself.
        assert!(room.kick_player("p1", "Host", 4).is_err());
        assert!(room.kick_player("w2", "Troll", 4).is_err());
        assert_eq!(
            room.kick_player("host", "HOST", 4).unwrap_err(),
            "You can't remove yourself"
        );
        assert!(room.kick_player("host", "Nobody", 4).is_err());
        // Apply refuses it: the host has sockets to close.
        assert!(
            room.apply(
                "host",
                ClientMessage::KickPlayer {
                    name: "Troll".into()
                },
                4
            )
            .is_err()
        );

        let outcome = room.kick_player("host", " troll ", 5).unwrap();
        assert_eq!(outcome.name, "Troll");
        assert_eq!(outcome.removed, vec![("p1".to_owned(), false)]);
        let snapshot = outcome.snapshot.expect("the lease was released");
        assert!(snapshot.revision > revision);
        assert!(snapshot.pieces.iter().all(|p| p.held_by.is_none()));
        assert!(snapshot.holders.iter().all(|h| h.owner != "Troll"));
        assert!(!room.names.contains_key("p1"));
        assert!(room.name_claims.iter().all(|c| c.name != "troll"));
        // Not under the name, not under another name with the same key, not
        // as a spectator, and not without a key.
        for (name, key) in [
            ("Troll", Some("new-key")),
            ("Totally New", Some("troll-key")),
            ("troll", None),
        ] {
            assert_eq!(
                room.admit_player("p2", name, key, 6).unwrap_err(),
                crate::REMOVED_BY_HOST
            );
        }
        assert_eq!(
            room.admit_spectator("w3", "Troll", None).unwrap_err(),
            crate::REMOVED_BY_HOST
        );
        // Everyone else is unaffected.
        room.admit_player("p3", "Friend", Some("f"), 6).unwrap();
        // The ban is saved with the room.
        let stored = StoredRoom {
            snapshot: room.snapshot.clone().unwrap(),
            name_claims: room.name_claims.clone(),
            timelapse: room.timelapse.clone(),
            bans: room.bans.clone(),
        };
        let back: StoredRoom =
            serde_json::from_str(&serde_json::to_string(&stored).unwrap()).unwrap();
        assert_eq!(back.bans, room.bans);
        assert_eq!(back.bans[0].key_hashes, ["troll-key"]);
    }

    #[test]
    fn kicking_a_spectator_bans_their_key_and_a_name_in_use_by_no_one_else() {
        let mut room = room(false);
        room.admit_spectator("w1", "Peeper", Some("peep-key"))
            .unwrap();
        let outcome = room.kick_player("host", "peeper", 1).unwrap();
        assert_eq!(outcome.removed, vec![("w1".to_owned(), true)]);
        assert!(outcome.snapshot.is_none(), "nothing on the board changed");
        assert!(room.spectators.is_empty());
        assert_eq!(
            room.admit_player("p", "Someone", Some("peep-key"), 2)
                .unwrap_err(),
            REMOVED_BY_HOST
        );
    }

    #[test]
    fn a_player_who_just_left_can_still_be_banned_through_their_claim() {
        let mut room = room(false);
        room.admit_player("p1", "Ada", Some("ada-key"), 1).unwrap();
        room.leave("p1", "Ada");
        let outcome = room.kick_player("host", "Ada", 2).unwrap();
        assert!(outcome.removed.is_empty());
        assert_eq!(
            room.admit_player("p2", "Ada", Some("ada-key"), 3)
                .unwrap_err(),
            REMOVED_BY_HOST
        );
        // A different player could not take the claimed name before either.
        assert_ne!(REMOVED_BY_HOST, NAME_TAKEN);
    }

    #[test]
    fn bans_are_bounded() {
        let mut room = room(false);
        for index in 0..(MAX_ROOM_BANS + 5) {
            let name = format!("P{index}");
            room.admit_player(&name, &name, Some(&name), 1).unwrap();
            room.kick_player("host", &name, 1).unwrap();
        }
        assert_eq!(room.bans.len(), MAX_ROOM_BANS);
        assert_eq!(room.bans[0].name, "p5", "the oldest bans went first");
    }
}
