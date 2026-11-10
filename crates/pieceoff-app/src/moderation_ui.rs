//! Player moderation in the room UI: mute (this device only), the host's
//! "Remove from room" with a rejoin ban, the host's chat switch, and
//! "Report room" for public rooms.

use super::*;
use crate::ui::DANGER;
use eframe::egui::RichText;

/// Where public-room reports go. No backend: the player's mail app opens.
pub(crate) const REPORT_ADDRESS: &str = "contact@pieceoff.app";

/// A `mailto:` link for reporting the public room `code`.
pub(crate) fn report_mailto(code: &str) -> String {
    let code: String = code.chars().filter(char::is_ascii_alphanumeric).collect();
    format!(
        "mailto:{REPORT_ADDRESS}?subject=Report%20PieceOff%20room%20{code}&body=Room%20code%3A%20{code}%0AWhat%20happened%3A%20"
    )
}

impl PieceOffApp {
    /// One other player (or spectator) in the player list: their name, a
    /// Mute toggle, and for the host, Remove.
    pub(crate) fn player_row(&mut self, ui: &mut egui::Ui, name: &str, is_host: bool) {
        let language = self.settings.language;
        ui.horizontal(|ui| {
            let muted = self.chat.is_muted(name);
            if muted {
                ui.weak(format!("{name} ({})", language.text("muted")));
            } else {
                ui.label(name);
            }
            let (label, hint) = if muted {
                ("Unmute", "Show this player's chat, pings and cursor again")
            } else {
                (
                    "Mute",
                    "Hide this player's chat, pings and cursor on this device",
                )
            };
            if ui
                .small_button(language.text(label))
                .on_hover_text(language.text(hint))
                .clicked()
            {
                self.chat.set_muted(name, !muted);
                if !muted {
                    let folded = name.to_lowercase();
                    let hidden: Vec<String> = self
                        .cursors
                        .remote
                        .iter()
                        .filter(|(_, cursor)| cursor.name.to_lowercase() == folded)
                        .map(|(id, _)| id.clone())
                        .collect();
                    for id in hidden {
                        self.cursors.hide(&id);
                    }
                }
            }
            if is_host
                && ui
                    .small_button(RichText::new(language.text("Remove…")).color(DANGER))
                    .on_hover_text(
                        language.text("Remove this player from the room. They can't rejoin it."),
                    )
                    .clicked()
            {
                self.confirm_kick = Some(name.to_owned());
                ui.close();
            }
        });
    }

    /// The host confirms removing a player.
    pub(crate) fn kick_window(&mut self, ctx: &egui::Context) {
        let Some(name) = self.confirm_kick.clone() else {
            return;
        };
        let language = self.settings.language;
        egui::Window::new(language.text("Remove player?"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(RichText::new(&name).strong());
                ui.label(language.text(
                    "They leave the room now and can't rejoin it, under this name or another, from this device.",
                ));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(RichText::new(language.text("Remove")).color(DANGER))
                        .clicked()
                    {
                        self.kick_player(&name);
                    }
                    if ui.button(language.text("Cancel")).clicked() {
                        self.confirm_kick = None;
                    }
                });
            });
    }

    pub(crate) fn kick_player(&mut self, name: &str) {
        self.confirm_kick = None;
        if !self.connected {
            self.status = "Not connected to the room.".into();
            return;
        }
        self.send(ClientMessage::KickPlayer { name: name.into() });
        self.status = format!("Removed {name} from the room.");
    }

    /// The host's room chat switch, in the More menu.
    pub(crate) fn chat_toggle(&mut self, ui: &mut egui::Ui, is_host: bool) {
        if !is_host || self.spectating {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let language = self.settings.language;
        let mut enabled = snapshot.chat_enabled;
        if ui
            .checkbox(&mut enabled, language.text("Room chat"))
            .on_hover_text(language.text(
                "Let everyone in the room chat. Public rooms start with chat off; pings always work.",
            ))
            .changed()
        {
            self.send(ClientMessage::SetChatEnabled { enabled });
        }
    }

    /// "Report room" for public rooms, for anyone in them.
    pub(crate) fn report_room_button(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(code) = self
            .snapshot
            .as_ref()
            .filter(|snapshot| snapshot.listed)
            .map(|snapshot| snapshot.invite_code.clone())
        else {
            return;
        };
        let language = self.settings.language;
        if ui
            .button(language.text("Report room…"))
            .on_hover_text(language.text(
                "Email PieceOff about abuse in this public room. The room code is filled in.",
            ))
            .clicked()
        {
            ui.close();
            ctx.open_url(egui::OpenUrl::new_tab(report_mailto(&code)));
            self.status = format!("Write to {REPORT_ADDRESS} about room {code}.");
        }
    }
}
