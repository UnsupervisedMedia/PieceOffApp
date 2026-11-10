use crate::{ChatLine, ClientMessage, RoomRuntime, ServerMessage, clean_chat_text};

impl RoomRuntime {
    /// Validate and stamp a message that does not change saved room state.
    /// Hosts enforce rate limits and retain recent chat around this shared rule.
    pub fn relay(
        &self,
        client_id: &str,
        message: ClientMessage,
        chat_id: u64,
        now_ms: u64,
    ) -> Result<Option<ServerMessage>, String> {
        let name = self
            .names
            .get(client_id)
            .or_else(|| self.spectators.get(client_id))
            .ok_or("Player is not in this room")?;
        let spectator = self.is_spectator(client_id);
        let bounds = self
            .snapshot
            .as_ref()
            .ok_or("Room is not ready")?
            .scene_bounds();
        let event = match message {
            ClientMessage::Cursor { x, y } if !spectator && bounds.contains(x, y) => {
                Some(ServerMessage::Cursor {
                    client_id: client_id.into(),
                    name: name.clone(),
                    x,
                    y,
                })
            }
            ClientMessage::CursorHidden if !spectator => Some(ServerMessage::CursorHidden {
                client_id: client_id.into(),
            }),
            ClientMessage::Cursor { .. } | ClientMessage::CursorHidden => None,
            ClientMessage::Chat { text } => clean_chat_text(&text).map(|text| {
                ServerMessage::Chat(ChatLine {
                    id: chat_id,
                    client_id: client_id.into(),
                    name: name.clone(),
                    text,
                    sent_at_ms: now_ms,
                })
            }),
            ClientMessage::Ping { .. } if spectator => {
                return Err("Spectators can chat but cannot ping the board.".into());
            }
            ClientMessage::Ping { x, y } if bounds.contains(x, y) => Some(ServerMessage::Ping {
                client_id: client_id.into(),
                name: name.clone(),
                x,
                y,
            }),
            ClientMessage::Ping { .. } => None,
            _ => return Err("Message is not a relay event".into()),
        };
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RoomSnapshot;

    fn room() -> RoomRuntime {
        let snapshot = RoomSnapshot::create(
            "room".into(),
            "INVITE".into(),
            "Puzzle".into(),
            "a".repeat(64),
            "png".into(),
            400,
            300,
            3,
            4,
            "a".into(),
        )
        .unwrap();
        let mut room = RoomRuntime::from_snapshot(snapshot);
        room.names.insert("a".into(), "Ada".into());
        room.spectators.insert("watch".into(), "Watcher".into());
        room
    }

    #[test]
    fn stamps_and_sanitizes_relay_messages() {
        let room = room();
        let event = room
            .relay(
                "a",
                ClientMessage::Chat {
                    text: " hi\nthere ".into(),
                },
                7,
                42,
            )
            .unwrap();
        let Some(ServerMessage::Chat(line)) = event else {
            panic!("expected chat")
        };
        assert_eq!(
            (
                line.id,
                line.name.as_str(),
                line.text.as_str(),
                line.sent_at_ms
            ),
            (7, "Ada", "hi there", 42)
        );
        assert!(
            room.relay(
                "a",
                ClientMessage::Cursor {
                    x: f32::NAN,
                    y: 0.0
                },
                0,
                0
            )
            .unwrap()
            .is_none()
        );
        assert!(matches!(
            room.relay("a", ClientMessage::Ping { x: 1.0, y: 2.0 }, 0, 0)
                .unwrap(),
            Some(ServerMessage::Ping { .. })
        ));
        assert!(
            room.relay("unknown", ClientMessage::Chat { text: "hi".into() }, 0, 0)
                .is_err()
        );
    }

    #[test]
    fn spectators_can_chat_but_cannot_point() {
        let room = room();
        assert!(
            room.relay("watch", ClientMessage::Chat { text: "hi".into() }, 1, 42)
                .unwrap()
                .is_some()
        );
        assert!(
            room.relay("watch", ClientMessage::Cursor { x: 0.0, y: 0.0 }, 0, 0)
                .unwrap()
                .is_none()
        );
        assert!(
            room.relay("watch", ClientMessage::Ping { x: 0.0, y: 0.0 }, 0, 0)
                .is_err()
        );
    }
}
