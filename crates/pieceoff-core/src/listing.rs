//! Built-in media and public ("listed") rooms (protocol v15).
//!
//! Every client ships the same bundled loops, so a queue item whose
//! fingerprint is one of them is available the moment it is queued: nobody
//! uploads it and nobody downloads it. A listed room can be found by
//! strangers, so it may only show built-in media, never a player's upload.

use crate::{RoomRuntime, RoomSnapshot};

/// The refusal for non-built-in media in a listed room.
pub const PUBLIC_ROOMS_BUILTIN_ONLY: &str = "Public rooms use built-in puzzles only";

/// The bundled loops (`crates/pieceoff-app/assets/builtin/*.webp`) as
/// `(sha256 hex, title)`. A test hashes the asset files and requires this
/// list to match them exactly, so replacing an asset means updating it here
/// (and bumping the protocol, since older clients would not have the file).
pub const BUILTIN_MEDIA_SHA256: &[(&str, &str)] = &[
    (
        "f50f079590abeae947fc87405072a1e8f75cfc8d64694726a75cc5d612f8db80",
        "Slow Orbit",
    ),
    (
        "06add36908c1cb4f50128d0c9133b38ceea20f873e2d0e1fae06e08795d07da5",
        "Tidal Glass",
    ),
    (
        "5d5120c8599dcb77e3c3953b65e5abffab114527bd79323b7dd1cdff9902b1b8",
        "Aurora Veil",
    ),
    (
        "40ecc031c280369707c58927eab00777323fbd6cf6d97cb3a1d831dbcc836eb3",
        "Dune Light",
    ),
    (
        "5408b1d0f453d263b4a4f5d6983a15655550dd1f45b71ee0b7d0fed0fead64ae",
        "Leaf Garden",
    ),
    (
        "c98c9143a6e86b3ed06995c144c30e03e9e08cd446b57a45cc996f506cc5cf8a",
        "Glass Garden",
    ),
    (
        "e0586f46ff010d759130e07aef4b232fe8d8a00efe4dfadf9b663611ba4835f0",
        "Deep Sea Bloom",
    ),
    (
        "1ea4b43428fb10e9d3c21df0c6f1940a7e6d9207f1fee90d22d249777e2cf00a",
        "Geometric Bloom",
    ),
];

/// The bundled title for a built-in fingerprint (case-insensitive hex), or
/// `None` for anything else. Lobbies show this, never a client's title.
pub fn builtin_media_title(sha256: &str) -> Option<&'static str> {
    BUILTIN_MEDIA_SHA256
        .iter()
        .find(|(hash, _)| hash.eq_ignore_ascii_case(sha256))
        .map(|(_, title)| *title)
}

/// Whether `sha256` is one of the bundled loops.
pub fn is_builtin_media(sha256: &str) -> bool {
    builtin_media_title(sha256).is_some()
}

impl RoomSnapshot {
    /// Whether every queue item (and the current-media mirror) is built-in,
    /// which a listed room requires.
    pub fn only_builtin_media(&self) -> bool {
        is_builtin_media(&self.media_sha256)
            && self.queue.iter().all(|item| is_builtin_media(&item.sha256))
    }

    /// Built-in items need no upload: mark them available. Other items are
    /// left as they are.
    pub(crate) fn mark_builtins_available(&mut self) {
        for item in &mut self.queue {
            if is_builtin_media(&item.sha256) {
                item.available = true;
            }
        }
        self.sync_current_mirror();
    }

    /// List or delist the room, refusing to list one with non-built-in media.
    pub(crate) fn try_set_listed(&mut self, listed: bool) -> Result<(), String> {
        if listed && !self.only_builtin_media() {
            return Err(PUBLIC_ROOMS_BUILTIN_ONLY.into());
        }
        let was_listed = self.listed;
        self.listed = listed;
        self.note_listing_change(was_listed);
        Ok(())
    }
}

impl RoomRuntime {
    /// Host-only: list the room publicly or take it off the list. Listing
    /// needs every queue item to be built-in. Not affected by the video lock.
    pub fn set_listed(&mut self, client_id: &str, listed: bool) -> Result<RoomSnapshot, String> {
        let snapshot = self.snapshot.as_mut().ok_or("Room is not initialized")?;
        if snapshot.host_id != client_id {
            return Err("Only the host can list or unlist the room".into());
        }
        // The host's name is what the public lobby shows.
        let host_name = self.names.get(client_id).map(String::as_str);
        if listed && !snapshot.listed && !host_name.is_some_and(crate::public_name_allowed) {
            return Err(crate::PUBLIC_NAME_REFUSED.into());
        }
        if snapshot.listed != listed {
            snapshot.try_set_listed(listed)?;
            snapshot.revision += 1;
        }
        Ok(snapshot.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClientMessage, ServerMessage};
    use sha2::{Digest, Sha256};
    use std::collections::BTreeSet;

    #[test]
    fn builtin_list_matches_the_bundled_assets_exactly() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../pieceoff-app/assets/builtin");
        let mut on_disk = BTreeSet::new();
        for entry in std::fs::read_dir(&dir).expect("built-in asset directory") {
            let path = entry.unwrap().path();
            if !path.is_file() {
                continue;
            }
            let title = path.file_stem().unwrap().to_str().unwrap().to_owned();
            let bytes = std::fs::read(&path).unwrap();
            let hash: String = Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            on_disk.insert((hash, title));
        }
        let listed: BTreeSet<_> = BUILTIN_MEDIA_SHA256
            .iter()
            .map(|(hash, title)| ((*hash).to_owned(), (*title).to_owned()))
            .collect();
        assert_eq!(listed.len(), BUILTIN_MEDIA_SHA256.len(), "duplicate entry");
        assert_eq!(listed, on_disk);
    }

    #[test]
    fn titles_are_found_case_insensitively() {
        let (hash, title) = BUILTIN_MEDIA_SHA256[0];
        assert_eq!(builtin_media_title(&hash.to_ascii_uppercase()), Some(title));
        assert!(!is_builtin_media(&"0".repeat(64)));
    }

    fn create(sha: &str, listed: bool) -> ClientMessage {
        ClientMessage::CreateRoom {
            name: "Host".into(),
            title: "Puzzle".into(),
            media_sha256: sha.into(),
            media_extension: "webp".into(),
            width: 480,
            height: 320,
            rows: 3,
            columns: 4,
            initial_snapshot: None,
            duration_ms: 0,
            player_key: String::new(),
            cut_style: Default::default(),
            listed,
        }
    }

    fn builtin(index: usize) -> &'static str {
        BUILTIN_MEDIA_SHA256[index].0
    }

    fn queue_add(sha: &str) -> ClientMessage {
        ClientMessage::QueueAdd {
            sha256: sha.into(),
            extension: "webp".into(),
            title: "Next".into(),
            width: 480,
            height: 320,
            duration_ms: 0,
        }
    }

    fn snapshot_of(event: ServerMessage) -> RoomSnapshot {
        match event {
            ServerMessage::Snapshot { snapshot } => snapshot,
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    #[test]
    fn a_builtin_room_is_available_at_once_and_may_be_listed() {
        let (room, _) =
            RoomRuntime::new_room("host", create(builtin(0), true), "id".into(), "CODE".into())
                .unwrap();
        let snapshot = room.snapshot.unwrap();
        assert!(snapshot.listed);
        assert!(snapshot.media_available);
        assert!(snapshot.queue[0].available);
    }

    #[test]
    fn an_uploaded_room_cannot_be_created_listed() {
        let error = RoomRuntime::new_room(
            "host",
            create(&"a".repeat(64), true),
            "id".into(),
            "CODE".into(),
        )
        .err()
        .unwrap();
        assert_eq!(error, PUBLIC_ROOMS_BUILTIN_ONLY);
        let (room, _) = RoomRuntime::new_room(
            "host",
            create(&"a".repeat(64), false),
            "id".into(),
            "CODE".into(),
        )
        .unwrap();
        let snapshot = room.snapshot.unwrap();
        assert!(!snapshot.listed);
        assert!(!snapshot.media_available, "uploads still need uploading");
    }

    #[test]
    fn a_listed_room_refuses_uploaded_media_and_takes_builtins_ready() {
        let (mut room, _) =
            RoomRuntime::new_room("host", create(builtin(0), true), "id".into(), "CODE".into())
                .unwrap();
        let error = room
            .apply("host", queue_add(&"b".repeat(64)), 0)
            .unwrap_err();
        assert_eq!(error, PUBLIC_ROOMS_BUILTIN_ONLY);
        let snapshot = snapshot_of(room.apply("host", queue_add(builtin(1)), 0).unwrap().event);
        assert_eq!(snapshot.queue.len(), 2);
        assert!(snapshot.queue[1].available);
    }

    #[test]
    fn set_listed_is_host_only_and_needs_builtin_media() {
        let (mut room, _) = RoomRuntime::new_room(
            "host",
            create(builtin(0), false),
            "id".into(),
            "CODE".into(),
        )
        .unwrap();
        room.names.insert("guest".into(), "Guest".into());
        assert!(
            room.apply("guest", ClientMessage::SetListed { listed: true }, 0)
                .is_err()
        );
        room.apply("host", queue_add(&"c".repeat(64)), 0).unwrap();
        let error = room
            .apply("host", ClientMessage::SetListed { listed: true }, 0)
            .unwrap_err();
        assert_eq!(error, PUBLIC_ROOMS_BUILTIN_ONLY);
        room.apply("host", ClientMessage::QueueRemove { index: 1 }, 0)
            .unwrap();
        let outcome = room
            .apply("host", ClientMessage::SetListed { listed: true }, 0)
            .unwrap();
        assert!(outcome.persist);
        assert!(snapshot_of(outcome.event).listed);
        let snapshot = snapshot_of(
            room.apply("host", ClientMessage::SetListed { listed: false }, 0)
                .unwrap()
                .event,
        );
        assert!(!snapshot.listed);
    }

    #[test]
    fn spectators_cannot_list_a_room() {
        let (mut room, _) = RoomRuntime::new_room(
            "host",
            create(builtin(0), false),
            "id".into(),
            "CODE".into(),
        )
        .unwrap();
        room.spectators.insert("watcher".into(), "Watcher".into());
        assert!(
            room.apply("watcher", ClientMessage::SetListed { listed: true }, 0)
                .is_err()
        );
        assert!(!room.snapshot.unwrap().listed);
    }

    #[test]
    fn listed_defaults_off_for_older_snapshots_and_messages() {
        let (room, _) = RoomRuntime::new_room(
            "host",
            create(builtin(0), false),
            "id".into(),
            "CODE".into(),
        )
        .unwrap();
        let mut json = serde_json::to_value(room.snapshot.unwrap()).unwrap();
        json.as_object_mut().unwrap().remove("listed");
        let snapshot: RoomSnapshot = serde_json::from_value(json).unwrap();
        assert!(!snapshot.listed);
        let mut message = serde_json::to_value(create(builtin(0), true)).unwrap();
        message.as_object_mut().unwrap().remove("listed");
        match serde_json::from_value(message).unwrap() {
            ClientMessage::CreateRoom { listed, .. } => assert!(!listed),
            _ => unreachable!(),
        }
    }

    #[test]
    fn a_damaged_listed_snapshot_is_delisted_on_load() {
        let (room, _) = RoomRuntime::new_room(
            "host",
            create(&"d".repeat(64), false),
            "id".into(),
            "CODE".into(),
        )
        .unwrap();
        let mut snapshot = room.snapshot.unwrap();
        snapshot.listed = true;
        let room = RoomRuntime::from_snapshot(snapshot);
        assert!(!room.snapshot.unwrap().listed);
    }
}
