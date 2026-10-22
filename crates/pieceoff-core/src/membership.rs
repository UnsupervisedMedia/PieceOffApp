use crate::{
    ClientMessage, RoomRuntime, RoomSnapshot, ServerMessage, clean_media_extension, clean_name,
    clean_title, valid_sha256,
};

/// Players allowed in one room.
pub const MAX_PLAYERS: usize = 8;

/// What joining changed that the host must act on.
#[derive(Debug)]
pub struct JoinOutcome {
    /// The cleaned display name.
    pub name: String,
    /// The recorded host was absent, so the joiner became host.
    pub promoted_to_host: bool,
    /// Promoted while media is still missing: the host must mint a fresh
    /// upload token for the new host.
    pub needs_upload_token: bool,
}

/// The pending upload a promoted host must finish.
#[derive(Debug, PartialEq, Eq)]
pub struct UploadAssignment {
    pub client_id: String,
    pub sha256: String,
}

/// What leaving changed. Events go out in this order: `PeerLeft`, then
/// `Snapshot` if present, then the upload token for `upload_assignment`.
#[derive(Debug)]
pub struct LeaveOutcome {
    pub name: String,
    pub snapshot: Option<RoomSnapshot>,
    pub upload_assignment: Option<UploadAssignment>,
}

fn has_unavailable_item(snapshot: &RoomSnapshot) -> bool {
    snapshot.queue.iter().any(|item| !item.available)
}

impl RoomRuntime {
    /// Build a new room from a `CreateRoom` message. The host supplies the
    /// room id and invite code, since core has no randomness. Returns the
    /// runtime (creator is host, named) and the creator's cleaned name.
    pub fn new_room(
        client_id: &str,
        message: ClientMessage,
        room_id: String,
        invite_code: String,
    ) -> Result<(RoomRuntime, String), String> {
        let ClientMessage::CreateRoom {
            name,
            title,
            media_sha256,
            media_extension,
            width,
            height,
            rows,
            columns,
            initial_snapshot,
            duration_ms,
        } = message
        else {
            return Err("First message must create or join a room".into());
        };
        if !valid_sha256(&media_sha256) {
            return Err("Invalid media fingerprint".into());
        }
        let name = clean_name(&name);
        let clean_title = clean_title(&title);
        let clean_extension = clean_media_extension(&media_extension)?;
        let snapshot = if let Some(initial) = initial_snapshot {
            let initial = *initial;
            if initial.title != clean_title
                || initial.media_sha256 != media_sha256
                || initial.media_extension != clean_extension
                || initial.width != width
                || initial.height != height
                || initial.rows != rows
                || initial.columns != columns
            {
                return Err("Imported puzzle snapshot does not match its media or grid".into());
            }
            initial.rehost(room_id, invite_code, client_id.to_owned())?
        } else {
            let mut snapshot = RoomSnapshot::create(
                room_id,
                invite_code,
                clean_title,
                media_sha256,
                clean_extension,
                width,
                height,
                rows,
                columns,
                client_id.to_owned(),
            )?;
            snapshot.revision = 1;
            if let Some(first) = snapshot.queue.first_mut() {
                first.duration_ms = duration_ms;
            }
            snapshot
        };
        let mut runtime = RoomRuntime::from_snapshot(snapshot);
        runtime.host_id = Some(client_id.to_owned());
        runtime.names.insert(client_id.to_owned(), name.clone());
        Ok((runtime, name))
    }

    /// Add a player. Promotes them to host when the recorded host is gone.
    pub fn join(
        &mut self,
        client_id: &str,
        name: &str,
        media_sha256: &str,
    ) -> Result<JoinOutcome, String> {
        let snapshot = self.snapshot.as_ref().ok_or("Room is not ready")?;
        if media_sha256 != snapshot.media_sha256 && !snapshot.media_available {
            return Err(
                "The room media is still uploading. Wait for the host and try joining again."
                    .into(),
            );
        }
        if !self.names.contains_key(client_id) && self.names.len() >= MAX_PLAYERS {
            return Err("This room already has 8 players".into());
        }
        let name = clean_name(name);
        self.names.insert(client_id.to_owned(), name.clone());
        let mut promoted_to_host = false;
        let mut needs_upload_token = false;
        if let Some(snapshot) = self.snapshot.as_mut()
            && !self.names.contains_key(&snapshot.host_id)
        {
            snapshot.host_id = client_id.to_owned();
            snapshot.revision += 1;
            promoted_to_host = true;
            needs_upload_token = has_unavailable_item(snapshot);
        }
        Ok(JoinOutcome {
            name,
            promoted_to_host,
            needs_upload_token,
        })
    }

    /// Remove a player: release their piece leases and hand the host role to
    /// someone else if they held it.
    pub fn leave(&mut self, client_id: &str, fallback_name: &str) -> LeaveOutcome {
        self.forget_drag(client_id);
        let name = self
            .names
            .remove(client_id)
            .unwrap_or_else(|| fallback_name.to_owned());
        let next_host = self.names.keys().next().cloned();
        let mut changed_snapshot = None;
        let mut upload_assignment = None;
        if let Some(snapshot) = self.snapshot.as_mut() {
            let mut changed = false;
            for piece in &mut snapshot.pieces {
                if piece.held_by.as_deref() == Some(client_id) {
                    piece.held_by = None;
                    changed = true;
                }
            }
            if snapshot.host_id == client_id
                && let Some(next) = next_host
            {
                snapshot.host_id = next.clone();
                changed = true;
                if let Some(pending) = snapshot.queue.iter().find(|item| !item.available) {
                    upload_assignment = Some(UploadAssignment {
                        client_id: next,
                        sha256: pending.sha256.clone(),
                    });
                }
            }
            if changed {
                snapshot.revision += 1;
                changed_snapshot = Some(snapshot.clone());
            }
        }
        LeaveOutcome {
            name,
            snapshot: changed_snapshot,
            upload_assignment,
        }
    }

    /// Media for `sha256` has finished uploading: mark every queue item with
    /// that fingerprint available. Returns the new snapshot to broadcast, or
    /// `None` if the room has no snapshot or no item has that fingerprint (so
    /// nothing changed and no revision is taken).
    pub fn mark_item_available(&mut self, sha256: &str) -> Option<RoomSnapshot> {
        let snapshot = self.snapshot.as_mut()?;
        let mut matched = false;
        for item in &mut snapshot.queue {
            if item.sha256 == sha256 {
                item.available = true;
                matched = true;
            }
        }
        if !matched {
            return None;
        }
        snapshot.sync_current_mirror();
        snapshot.revision = snapshot.revision.saturating_add(1);
        Some(snapshot.clone())
    }

    /// The `Welcome` for a client that has just joined. `media_upload_token`
    /// is only included for the host while media is still missing.
    pub fn welcome(
        &self,
        client_id: &str,
        name: &str,
        media_upload_token: Option<String>,
    ) -> Result<ServerMessage, String> {
        let snapshot = self.snapshot.clone().ok_or("Room is not ready")?;
        let peers = self
            .names
            .iter()
            .filter(|(id, _)| id.as_str() != client_id)
            .map(|(_, name)| name.clone())
            .collect();
        let media_upload_token = if snapshot.host_id == client_id && has_unavailable_item(&snapshot)
        {
            media_upload_token
        } else {
            None
        };
        Ok(ServerMessage::Welcome {
            client_id: client_id.to_owned(),
            name: name.to_owned(),
            peers,
            snapshot,
            media_upload_token,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{ClientMessage, RoomRuntime, ServerMessage};

    fn sha() -> String {
        "a".repeat(64)
    }

    fn create_message(media_sha256: String) -> ClientMessage {
        ClientMessage::CreateRoom {
            name: "Host".into(),
            title: "Puzzle".into(),
            media_sha256,
            media_extension: "mp4".into(),
            width: 640,
            height: 360,
            rows: 3,
            columns: 4,
            initial_snapshot: None,
            duration_ms: 5000,
        }
    }

    fn room() -> RoomRuntime {
        RoomRuntime::new_room(
            "host",
            create_message(sha()),
            "room-id".into(),
            "ABCDEFGH".into(),
        )
        .unwrap()
        .0
    }

    #[test]
    fn new_room_makes_the_creator_host_at_revision_one() {
        let (runtime, name) = RoomRuntime::new_room(
            "host",
            create_message(sha()),
            "room-id".into(),
            "ABCDEFGH".into(),
        )
        .unwrap();
        let snapshot = runtime.snapshot.unwrap();
        assert_eq!(name, "Host");
        assert_eq!(snapshot.host_id, "host");
        assert_eq!(snapshot.revision, 1);
        assert_eq!(snapshot.invite_code, "ABCDEFGH");
        assert_eq!(runtime.names.get("host").map(String::as_str), Some("Host"));
        assert_eq!(snapshot.queue[0].duration_ms, 5000);
    }

    #[test]
    fn new_room_rejects_bad_input() {
        let bad_sha =
            RoomRuntime::new_room("h", create_message("nope".into()), "i".into(), "C".into());
        assert_eq!(bad_sha.err().as_deref(), Some("Invalid media fingerprint"));
        let join = ClientMessage::JoinRoom {
            invite_code: "X".into(),
            name: "n".into(),
            media_sha256: sha(),
            media_extension: "mp4".into(),
        };
        let wrong = RoomRuntime::new_room("h", join, "i".into(), "C".into());
        assert_eq!(
            wrong.err().as_deref(),
            Some("First message must create or join a room")
        );
        let mut message = create_message(sha());
        if let ClientMessage::CreateRoom {
            initial_snapshot, ..
        } = &mut message
        {
            let mut other = room().snapshot.unwrap();
            other.title = "Different".into();
            *initial_snapshot = Some(Box::new(other));
        }
        let mismatch = RoomRuntime::new_room("h", message, "i".into(), "C".into());
        assert_eq!(
            mismatch.err().as_deref(),
            Some("Imported puzzle snapshot does not match its media or grid")
        );
    }

    #[test]
    fn join_adds_a_player_and_caps_the_room_at_eight() {
        let mut runtime = room();
        for index in 1..8 {
            runtime.join(&format!("p{index}"), "P", &sha()).unwrap();
        }
        let err = runtime.join("p8", "P", &sha()).unwrap_err();
        assert_eq!(err, "This room already has 8 players");
        assert_eq!(runtime.names.len(), 8);
    }

    #[test]
    fn join_refuses_a_stranger_while_the_media_is_still_uploading() {
        let mut runtime = room();
        let err = runtime.join("p1", "P", &"b".repeat(64)).unwrap_err();
        assert!(
            err.starts_with("The room media is still uploading"),
            "{err}"
        );
        assert!(!runtime.names.contains_key("p1"));
    }

    #[test]
    fn join_promotes_when_the_recorded_host_is_absent() {
        let mut runtime = room();
        runtime.names.remove("host");
        let revision = runtime.snapshot.as_ref().unwrap().revision;
        let outcome = runtime.join("p1", "  Pat\n", &sha()).unwrap();
        let snapshot = runtime.snapshot.as_ref().unwrap();
        assert_eq!(outcome.name, "Pat");
        assert!(outcome.promoted_to_host);
        assert!(outcome.needs_upload_token);
        assert_eq!(snapshot.host_id, "p1");
        assert_eq!(snapshot.revision, revision + 1);
    }

    #[test]
    fn join_does_not_promote_when_the_host_is_present() {
        let mut runtime = room();
        let outcome = runtime.join("p1", "Pat", &sha()).unwrap();
        assert!(!outcome.promoted_to_host);
        assert!(!outcome.needs_upload_token);
    }

    #[test]
    fn leave_releases_leases_and_bumps_the_revision_once() {
        let mut runtime = room();
        runtime.join("p1", "Pat", &sha()).unwrap();
        runtime
            .apply("p1", ClientMessage::DragStart { piece_id: 0 }, 0)
            .unwrap();
        let before = runtime.snapshot.as_ref().unwrap().revision;
        let outcome = runtime.leave("p1", "fallback");
        let snapshot = outcome.snapshot.unwrap();
        assert_eq!(outcome.name, "Pat");
        assert_eq!(snapshot.revision, before + 1);
        assert!(snapshot.pieces.iter().all(|piece| piece.held_by.is_none()));
        assert!(outcome.upload_assignment.is_none());
        assert!(!runtime.names.contains_key("p1"));
    }

    #[test]
    fn leave_without_changes_returns_no_snapshot() {
        let mut runtime = room();
        runtime.join("p1", "Pat", &sha()).unwrap();
        let before = runtime.snapshot.as_ref().unwrap().revision;
        let outcome = runtime.leave("p1", "fallback");
        assert!(outcome.snapshot.is_none());
        assert_eq!(runtime.snapshot.unwrap().revision, before);
    }

    #[test]
    fn leave_uses_the_fallback_name_for_an_unknown_client() {
        let mut runtime = room();
        let outcome = runtime.leave("ghost", "Fallback");
        assert_eq!(outcome.name, "Fallback");
    }

    #[test]
    fn host_leaving_promotes_a_peer_and_assigns_the_pending_upload() {
        let mut runtime = room();
        runtime.join("p1", "Pat", &sha()).unwrap();
        let outcome = runtime.leave("host", "Host");
        let snapshot = outcome.snapshot.unwrap();
        assert_eq!(snapshot.host_id, "p1");
        let assignment = outcome.upload_assignment.unwrap();
        assert_eq!(assignment.client_id, "p1");
        assert_eq!(assignment.sha256, sha());
    }

    #[test]
    fn welcome_lists_other_players_and_withholds_the_token_from_non_hosts() {
        let mut runtime = room();
        runtime.join("p1", "Pat", &sha()).unwrap();
        let ServerMessage::Welcome {
            peers,
            media_upload_token,
            ..
        } = runtime.welcome("host", "Host", Some("tok".into())).unwrap()
        else {
            panic!("expected welcome");
        };
        assert_eq!(peers, vec!["Pat".to_string()]);
        assert_eq!(media_upload_token.as_deref(), Some("tok"));
        let ServerMessage::Welcome {
            media_upload_token, ..
        } = runtime.welcome("p1", "Pat", Some("tok".into())).unwrap()
        else {
            panic!("expected welcome");
        };
        assert_eq!(media_upload_token, None);
    }

    #[test]
    fn welcome_on_an_empty_runtime_is_an_error() {
        let err = RoomRuntime::default().welcome("a", "A", None).unwrap_err();
        assert_eq!(err, "Room is not ready");
    }

    #[test]
    fn a_player_can_rejoin_with_their_own_id_when_the_room_is_full() {
        let mut runtime = room();
        for index in 1..8 {
            runtime.join(&format!("p{index}"), "P", &sha()).unwrap();
        }
        runtime.join("p3", "Renamed", &sha()).unwrap();
        assert_eq!(runtime.names.len(), 8);
        assert_eq!(runtime.names.get("p3").map(String::as_str), Some("Renamed"));
    }

    #[test]
    fn mark_item_available_flags_the_item_and_bumps_the_revision() {
        let mut runtime = room();
        let before = runtime.snapshot.as_ref().unwrap().revision;
        let snapshot = runtime.mark_item_available(&sha()).unwrap();
        assert!(snapshot.queue[0].available);
        assert!(snapshot.media_available, "the current-item mirror follows");
        assert_eq!(snapshot.revision, before + 1);
        assert_eq!(runtime.snapshot.unwrap().revision, before + 1);
    }

    #[test]
    fn mark_item_available_without_a_room_is_none() {
        assert!(RoomRuntime::default().mark_item_available(&sha()).is_none());
    }

    #[test]
    fn mark_item_available_is_none_when_nothing_matches() {
        let mut runtime = room();
        let before = runtime.snapshot.as_ref().unwrap().revision;
        assert!(runtime.mark_item_available(&"f".repeat(64)).is_none());
        assert_eq!(
            runtime.snapshot.unwrap().revision,
            before,
            "no revision is taken"
        );
    }
}
