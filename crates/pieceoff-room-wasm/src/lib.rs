use pieceoff_core::{ClientMessage, RoomRuntime, RoomSnapshot, ServerMessage};
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// JS timestamps arrive as f64. Anything that is not a sane, non-negative
/// integer-valued time (NaN, infinity, negative) becomes 0 so core never does
/// arithmetic on it; the upper clamp is JS's largest exact integer.
fn clock(now_ms: f64) -> u64 {
    if now_ms.is_finite() && now_ms >= 0.0 {
        now_ms.min(9_007_199_254_740_991.0) as u64
    } else {
        0
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Reply {
    Ok { event: ServerMessage, persist: bool },
    Err(String),
}

fn reply(reply: Reply) -> String {
    serde_json::to_string(&reply).unwrap_or_else(|_| r#"{"err":"encode failed"}"#.into())
}

/// One room's rules, driven by JSON strings so a JavaScript Durable Object
/// can host it. Methods never throw except `from_snapshot` on unreadable JSON.
#[wasm_bindgen]
pub struct Room {
    runtime: RoomRuntime,
}

#[wasm_bindgen]
impl Room {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Room {
        Room {
            runtime: RoomRuntime::default(),
        }
    }

    pub fn from_snapshot(snapshot_json: &str) -> Result<Room, JsError> {
        let snapshot: RoomSnapshot =
            serde_json::from_str(snapshot_json).map_err(|e| JsError::new(&e.to_string()))?;
        Ok(Room {
            runtime: RoomRuntime::from_snapshot(snapshot),
        })
    }

    pub fn snapshot_json(&self) -> String {
        serde_json::to_string(&self.runtime.snapshot).unwrap_or_else(|_| "null".into())
    }

    pub fn handle(&mut self, client_id: &str, message_json: &str, now_ms: f64) -> String {
        let message: ClientMessage = match serde_json::from_str(message_json) {
            Ok(message) => message,
            Err(e) => return reply(Reply::Err(format!("Bad message: {e}"))),
        };
        match self.runtime.apply(client_id, message, clock(now_ms)) {
            Ok(out) => reply(Reply::Ok {
                event: out.event,
                persist: out.persist,
            }),
            Err(message) => reply(Reply::Err(message)),
        }
    }

    /// The `Snapshot` event as JSON when the queue auto-advanced.
    pub fn tick(&mut self, now_ms: f64) -> Option<String> {
        self.runtime
            .tick_advance(clock(now_ms))
            .and_then(|snapshot| serde_json::to_string(&ServerMessage::Snapshot { snapshot }).ok())
    }
}

impl Default for Room {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn created() -> Room {
        let snapshot = pieceoff_core::RoomSnapshot::create(
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
        Room::from_snapshot(&serde_json::to_string(&snapshot).unwrap()).unwrap()
    }

    #[test]
    fn handle_applies_a_message_and_returns_the_event() {
        let mut room = created();
        let reply = room.handle("host", r#"{"type":"drag_start","piece_id":0}"#, 0.0);
        assert!(reply.contains(r#""ok""#), "{reply}");
        assert!(reply.contains(r#""persist":false"#), "{reply}");
    }

    #[test]
    fn handle_reports_rule_errors_as_err_replies() {
        let mut room = Room::new();
        let reply = room.handle("host", r#"{"type":"drag_start","piece_id":0}"#, 0.0);
        assert_eq!(reply, r#"{"err":"Room is not initialized"}"#);
    }

    #[test]
    fn garbage_json_is_an_error_reply_not_a_panic() {
        let mut room = created();
        for bad in ["", "{", "null", r#"{"type":"nope"}"#, "\u{0}"] {
            let reply = room.handle("host", bad, 0.0);
            assert!(reply.starts_with(r#"{"err""#), "{bad:?} -> {reply}");
        }
    }

    #[test]
    fn non_finite_time_does_not_trap() {
        let mut room = created();
        for now in [f64::NAN, f64::INFINITY, -1.0] {
            let reply = room.handle("host", r#"{"type":"drag_start","piece_id":1}"#, now);
            assert!(reply.starts_with('{'), "{now} -> {reply}");
        }
    }

    #[test]
    fn snapshot_round_trips() {
        let room = created();
        let json = room.snapshot_json();
        let again = Room::from_snapshot(&json).unwrap();
        assert_eq!(again.snapshot_json(), json);
    }

    #[test]
    fn tick_is_quiet_when_nothing_is_due() {
        let mut room = created();
        assert!(room.tick(0.0).is_none());
    }
}
