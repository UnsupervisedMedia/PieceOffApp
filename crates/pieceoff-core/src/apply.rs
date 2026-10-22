use crate::{
    ClientMessage, MediaItem, RoomRuntime, RoomSnapshot, ServerMessage, clean_media_extension,
    clean_title,
};

/// What one applied client message produced.
#[derive(Debug)]
pub struct ApplyOutcome {
    pub event: ServerMessage,
    /// Whether the host should persist the room after broadcasting `event`.
    pub persist: bool,
}

impl ServerMessage {
    /// Whether `client_id` may see this message. Only upload tokens are
    /// addressed; everything else is room-wide.
    pub fn visible_to(&self, client_id: &str) -> bool {
        match self {
            ServerMessage::MediaUploadToken {
                client_id: recipient,
                ..
            } => recipient == client_id,
            // A player's own pointer is never echoed back.
            ServerMessage::Cursor {
                client_id: owner, ..
            }
            | ServerMessage::CursorHidden { client_id: owner } => owner != client_id,
            _ => true,
        }
    }
}

impl RoomRuntime {
    /// A runtime holding `snapshot`, with no names, host or in-flight drags.
    /// Hosts that rebuild a room from storage start here. The snapshot is
    /// migrated first, exactly as the axum server does on load, so a stored
    /// older or damaged snapshot cannot index outside its queue.
    pub fn from_snapshot(mut snapshot: RoomSnapshot) -> Self {
        snapshot.migrate();
        Self {
            snapshot: Some(snapshot),
            ..Self::default()
        }
    }

    /// Apply one message from `client_id`. The caller must hold whatever lock
    /// serialises this room and broadcast `event` before releasing it, so
    /// events leave in revision order.
    pub fn apply(
        &mut self,
        client_id: &str,
        message: ClientMessage,
        now_ms: u64,
    ) -> Result<ApplyOutcome, String> {
        if self.is_spectator(client_id) {
            return Err("You are watching this room; only players can change it.".into());
        }
        // Settling changes are what the timelapse records; the layout just
        // before the first pickup is where it starts.
        let settles = matches!(
            message,
            ClientMessage::DragEnd { .. }
                | ClientMessage::DragEndSelection { .. }
                | ClientMessage::RearrangeTray { .. }
                | ClientMessage::UpdateHolder { .. }
        );
        let restarts = matches!(message, ClientMessage::RestartPuzzle);
        if settles
            || matches!(
                message,
                ClientMessage::DragStart { .. } | ClientMessage::DragStartSelection { .. }
            )
        {
            self.start_timelapse();
        }
        let stats_before = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.stats.clone());
        let holders_before = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.holders.clone());
        let settled_before = self
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.settled_pieces());
        let (mut event, persist) = match message {
            ClientMessage::DragStart { piece_id } => {
                let (pieces, revision) = self.acquire(client_id, piece_id)?;
                self.note_pickup(now_ms);
                (pieces_changed(pieces, revision), false)
            }
            ClientMessage::DragStartSelection { piece_ids } => {
                let (pieces, revision) = self.acquire_selection(client_id, &piece_ids)?;
                self.note_pickup(now_ms);
                (pieces_changed(pieces, revision), false)
            }
            ClientMessage::MovePiece { piece_id, x, y } => {
                let (pieces, revision) = self.move_piece(client_id, piece_id, x, y, false)?;
                (pieces_changed(pieces, revision), false)
            }
            ClientMessage::DragEnd { piece_id, x, y } => {
                let (pieces, revision) = self.move_piece(client_id, piece_id, x, y, true)?;
                self.note_drop(client_id, settled_before, now_ms);
                (pieces_changed(pieces, revision), true)
            }
            ClientMessage::MoveSelection {
                anchor_piece_id,
                x,
                y,
            } => {
                let (pieces, revision) =
                    self.move_selection(client_id, anchor_piece_id, x, y, false)?;
                (pieces_changed(pieces, revision), false)
            }
            ClientMessage::DragEndSelection {
                anchor_piece_id,
                x,
                y,
            } => {
                let (pieces, revision) =
                    self.move_selection(client_id, anchor_piece_id, x, y, true)?;
                self.note_drop(client_id, settled_before, now_ms);
                (pieces_changed(pieces, revision), true)
            }
            ClientMessage::SetPlayback {
                playing,
                position_ms,
                rate,
                loop_start_ms,
                loop_end_ms,
            } => {
                let (playback, revision) = self.set_playback(
                    client_id,
                    playing,
                    position_ms,
                    rate,
                    loop_start_ms,
                    loop_end_ms,
                    now_ms,
                )?;
                (ServerMessage::PlaybackChanged { playback, revision }, true)
            }
            ClientMessage::RestartPuzzle => {
                let (pieces, revision) = self.restart(client_id, now_ms as u32)?;
                (pieces_changed(pieces, revision), true)
            }
            ClientMessage::RearrangeTray { arrangement } => {
                let (pieces, revision) = self.rearrange_tray(arrangement, now_ms as u32)?;
                (pieces_changed(pieces, revision), true)
            }
            ClientMessage::QueueAdd {
                sha256,
                extension,
                title,
                width,
                height,
                duration_ms,
            } => {
                let item = MediaItem {
                    sha256: sha256.to_ascii_lowercase(),
                    extension: clean_media_extension(&extension)?,
                    title: clean_title(&title),
                    width,
                    height,
                    duration_ms,
                    available: false,
                    position_ms: 0,
                    loop_start_ms: 0,
                    loop_end_ms: 0,
                };
                let snapshot = self.queue_add(client_id, item)?;
                (ServerMessage::Snapshot { snapshot }, true)
            }
            ClientMessage::QueueRemove { index } => {
                let snapshot = self.queue_remove(client_id, index, now_ms)?;
                (ServerMessage::Snapshot { snapshot }, true)
            }
            ClientMessage::QueueMove { from, to } => {
                let snapshot = self.queue_move(client_id, from, to)?;
                (ServerMessage::Snapshot { snapshot }, true)
            }
            ClientMessage::SelectItem { index } => {
                let snapshot = self.select_item(client_id, index, now_ms)?;
                (ServerMessage::Snapshot { snapshot }, true)
            }
            ClientMessage::NextItem => {
                let snapshot = self.step_item(client_id, true, now_ms)?;
                (ServerMessage::Snapshot { snapshot }, true)
            }
            ClientMessage::PrevItem => {
                let snapshot = self.step_item(client_id, false, now_ms)?;
                (ServerMessage::Snapshot { snapshot }, true)
            }
            ClientMessage::SetRepeat { mode } => {
                let snapshot = self.set_repeat(client_id, mode)?;
                (ServerMessage::Snapshot { snapshot }, true)
            }
            ClientMessage::SetVideoLock { locked } => {
                let snapshot = self.set_video_lock(client_id, locked)?;
                (ServerMessage::Snapshot { snapshot }, true)
            }
            ClientMessage::CreateHolder {
                x,
                y,
                width,
                height,
                label,
            } => {
                let revision = self.create_holder(client_id, x, y, width, height, &label)?;
                (pieces_changed(Vec::new(), revision), true)
            }
            ClientMessage::UpdateHolder {
                holder_id,
                x,
                y,
                width,
                height,
                label,
            } => {
                let (pieces, revision) =
                    self.update_holder(client_id, holder_id, x, y, width, height, &label)?;
                (pieces_changed(pieces, revision), true)
            }
            ClientMessage::DeleteHolder { holder_id } => {
                let revision = self.delete_holder(client_id, holder_id)?;
                (pieces_changed(Vec::new(), revision), true)
            }
            ClientMessage::CreateRoom { .. } | ClientMessage::JoinRoom { .. } => {
                return Err("Already joined a room".into());
            }
            // Hosts answer this to the asker alone, rate-limited.
            ClientMessage::RequestTimelapse => {
                return Err("Timelapse requests are answered by the host".into());
            }
            // Hosts relay these before applying; they are never room state.
            ClientMessage::Cursor { .. }
            | ClientMessage::CursorHidden
            | ClientMessage::Chat { .. }
            | ClientMessage::Ping { .. } => {
                return Err("Cursors, chat and pings are relayed, not applied".into());
            }
        };
        // Piece changes carry the stats whenever they moved, so clients never
        // need a per-second clock message.
        // Holders ride along the same way, so a restart that clears them or
        // a holder change reaches everyone in the one event.
        if let ServerMessage::PiecesChanged { pieces, .. } = &event {
            if restarts {
                self.timelapse = crate::Timelapse::default();
            } else if settles {
                self.record_timelapse(pieces, now_ms);
            }
        }
        if let ServerMessage::PiecesChanged { stats, holders, .. } = &mut event {
            let now = self.snapshot.as_ref().map(|snapshot| &snapshot.stats);
            if now != stats_before.as_ref() {
                *stats = now.cloned();
            }
            let now = self.snapshot.as_ref().map(|snapshot| &snapshot.holders);
            if now != holders_before.as_ref() {
                *holders = now.cloned();
            }
        }
        Ok(ApplyOutcome { event, persist })
    }
}

fn pieces_changed(pieces: Vec<crate::Piece>, revision: u64) -> ServerMessage {
    ServerMessage::PiecesChanged {
        pieces,
        revision,
        stats: None,
        holders: None,
    }
}

#[cfg(test)]
mod tests {
    use crate::{ClientMessage, RoomRuntime, RoomSnapshot, ServerMessage};
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
            names: HashMap::from([("host".into(), "Host".into())]),
            host_id: Some("host".into()),
            ..RoomRuntime::default()
        }
    }

    #[test]
    fn drag_start_leases_the_piece_and_does_not_persist() {
        let mut room = room();
        let out = room
            .apply("host", ClientMessage::DragStart { piece_id: 0 }, 0)
            .unwrap();
        assert!(!out.persist);
        assert!(matches!(out.event, ServerMessage::PiecesChanged { .. }));
    }

    #[test]
    fn drag_end_persists() {
        let mut room = room();
        room.apply("host", ClientMessage::DragStart { piece_id: 0 }, 0)
            .unwrap();
        let out = room
            .apply(
                "host",
                ClientMessage::DragEnd {
                    piece_id: 0,
                    x: 10.0,
                    y: 10.0,
                },
                0,
            )
            .unwrap();
        assert!(out.persist);
    }

    #[test]
    fn uninitialised_room_is_an_error_not_a_panic() {
        let mut room = RoomRuntime::default();
        let err = room
            .apply("host", ClientMessage::DragStart { piece_id: 0 }, 0)
            .unwrap_err();
        assert_eq!(err, "Room is not initialized");
    }

    #[test]
    fn create_and_join_are_rejected_once_in_a_room() {
        let mut room = room();
        let err = room
            .apply(
                "host",
                ClientMessage::JoinRoom {
                    invite_code: "x".into(),
                    name: "n".into(),
                    media_sha256: "hash".into(),
                    media_extension: "png".into(),
                    spectator: false,
                    player_key: String::new(),
                },
                0,
            )
            .unwrap_err();
        assert_eq!(err, "Already joined a room");
    }

    #[test]
    fn non_finite_coordinates_do_not_change_the_room() {
        let mut room = room();
        room.apply("host", ClientMessage::DragStart { piece_id: 0 }, 0)
            .unwrap();
        let before = room.snapshot.as_ref().unwrap().clone();
        let result = room.apply(
            "host",
            ClientMessage::MovePiece {
                piece_id: 0,
                x: f32::NAN,
                y: 0.0,
            },
            0,
        );
        assert!(result.is_err());
        assert_eq!(room.snapshot.unwrap(), before);
    }

    #[test]
    fn upload_tokens_are_visible_only_to_their_recipient() {
        let token = ServerMessage::MediaUploadToken {
            client_id: "a".into(),
            token: "t".into(),
            sha256: None,
        };
        assert!(token.visible_to("a"));
        assert!(!token.visible_to("b"));
        let other = ServerMessage::PeerLeft {
            name: "n".into(),
            spectator: false,
        };
        assert!(other.visible_to("b"));
    }

    fn old_format_snapshot() -> RoomSnapshot {
        let mut snapshot = room().snapshot.unwrap();
        snapshot.queue.clear();
        snapshot.protocol_version = 7;
        snapshot
    }

    #[test]
    fn from_snapshot_migrates_old_snapshots_so_queue_steps_do_not_panic() {
        let mut runtime = RoomRuntime::from_snapshot(old_format_snapshot());
        for message in [ClientMessage::NextItem, ClientMessage::PrevItem] {
            runtime.apply("host", message, 0).unwrap();
        }
        assert_eq!(runtime.snapshot.unwrap().queue.len(), 1);
    }

    #[test]
    fn stepping_an_empty_queue_is_an_error_not_a_panic() {
        let mut snapshot = old_format_snapshot();
        snapshot.media_sha256.clear();
        let mut runtime = RoomRuntime {
            snapshot: Some(snapshot),
            ..RoomRuntime::default()
        };
        for message in [ClientMessage::NextItem, ClientMessage::PrevItem] {
            assert!(runtime.apply("host", message, 0).is_err());
        }
    }

    #[test]
    fn from_snapshot_clamps_an_out_of_range_current_item() {
        let mut snapshot = room().snapshot.unwrap();
        snapshot.migrate();
        snapshot.current = 5;
        snapshot.repeat = crate::RepeatMode::All;
        snapshot.playback.playing = true;
        snapshot.playback.loop_end_ms = 1;
        let mut runtime = RoomRuntime::from_snapshot(snapshot);
        runtime.tick_advance(u64::MAX / 2);
        assert_eq!(runtime.snapshot.unwrap().current, 0);
    }

    #[test]
    fn from_snapshot_builds_a_runtime_that_applies_messages() {
        let snapshot = room().snapshot.unwrap();
        let mut runtime = RoomRuntime::from_snapshot(snapshot.clone());
        assert_eq!(runtime.snapshot.as_ref(), Some(&snapshot));
        runtime
            .apply("host", ClientMessage::DragStart { piece_id: 0 }, 0)
            .unwrap();
    }
}
