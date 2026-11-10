use super::*;

/// A keyboard move uses the same group clamp and server drag commands as a
/// pointer move. The server still decides whether the piece snaps or joins.
fn nudge_target(snapshot: &RoomSnapshot, piece_id: u32, dx: f32, dy: f32) -> Option<(f32, f32)> {
    let anchor = snapshot.piece(piece_id)?;
    if anchor.placed || anchor.held_by.is_some() {
        return None;
    }
    let origins: Vec<_> = snapshot
        .pieces
        .iter()
        .filter(|piece| piece.group_id == anchor.group_id)
        .map(|piece| (piece.x, piece.y))
        .collect();
    let (dx, dy) = snapshot.clamp_move(origins, dx, dy);
    (dx != 0.0 || dy != 0.0).then_some((anchor.x + dx, anchor.y + dy))
}

impl PieceOffApp {
    fn keyboard_nudge(&mut self, piece_id: u32, dx: f32, dy: f32) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let Some(piece) = snapshot.piece(piece_id) else {
            return;
        };
        if snapshot
            .reserved_from(piece.group_id, Some(&self.player_name))
            .is_some()
        {
            self.status = "That piece is in another player's holder.".into();
            return;
        }
        let Some((x, y)) = nudge_target(snapshot, piece_id, dx, dy) else {
            return;
        };
        self.send(ClientMessage::DragStartSelection {
            piece_ids: vec![piece_id],
        });
        self.send(ClientMessage::DragEndSelection {
            anchor_piece_id: piece_id,
            x,
            y,
        });
    }

    pub(crate) fn keyboard_play_window(&mut self, ctx: &egui::Context) {
        if !self.keyboard_play_open {
            return;
        }
        let mut open = true;
        let language = self.settings.language;
        let ids: Vec<u32> = self.snapshot.as_ref().map_or_else(Vec::new, |snapshot| {
            snapshot
                .pieces
                .iter()
                .filter(|piece| {
                    !piece.placed
                        && piece.held_by.is_none()
                        && snapshot
                            .reserved_from(piece.group_id, Some(&self.player_name))
                            .is_none()
                })
                .map(|piece| piece.id)
                .collect()
        });
        let selected = self
            .keyboard_piece_id
            .filter(|id| ids.contains(id))
            .or_else(|| ids.first().copied());
        self.keyboard_piece_id = selected;
        let mut command = None;
        egui::Window::new(language.text("Keyboard play"))
            .open(&mut open)
            .default_width(280.0)
            .show(ctx, |ui| {
                ui.label(language.text("Use Tab and Enter to choose a piece and move it. The room server decides when it snaps into place."));
                let Some(id) = selected else {
                    ui.label(language.text("No loose pieces are available."));
                    return;
                };
                let index = ids.iter().position(|piece| *piece == id).unwrap_or(0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(index > 0, egui::Button::new(language.text("Previous piece"))).clicked() {
                        self.keyboard_piece_id = Some(ids[index - 1]);
                    }
                    ui.label(match language {
                        Language::English => format!("Piece {} of {}", index + 1, ids.len()),
                        Language::Spanish => format!("Pieza {} de {}", index + 1, ids.len()),
                    });
                    if ui.add_enabled(index + 1 < ids.len(), egui::Button::new(language.text("Next piece"))).clicked() {
                        self.keyboard_piece_id = Some(ids[index + 1]);
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button(language.text("Move up")).clicked() { command = Some((0.0, -1.0)); }
                    if ui.button(language.text("Move down")).clicked() { command = Some((0.0, 1.0)); }
                    if ui.button(language.text("Move left")).clicked() { command = Some((-1.0, 0.0)); }
                    if ui.button(language.text("Move right")).clicked() { command = Some((1.0, 0.0)); }
                });
            });
        self.keyboard_play_open = open;
        if let Some(id) = self.keyboard_piece_id {
            self.selected_pieces.clear();
            self.selected_pieces.insert(id);
        }
        if let (Some(id), Some((sx, sy)), Some(snapshot)) =
            (selected, command, self.snapshot.as_ref())
        {
            let step = (snapshot.cell_width().min(snapshot.cell_height()) / 4.0).max(1.0);
            self.keyboard_nudge(id, sx * step, sy * step);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_nudge_stops_at_table_bounds() {
        let mut snapshot = RoomSnapshot::create(
            "room".into(),
            "CODE".into(),
            "Test".into(),
            "a".repeat(64),
            "png".into(),
            400,
            300,
            3,
            4,
            "host".into(),
        )
        .unwrap();
        let bounds = snapshot.scene_bounds();
        let piece = snapshot
            .pieces
            .iter_mut()
            .find(|piece| piece.id == 0)
            .unwrap();
        piece.x = bounds.max_x;
        assert!(nudge_target(&snapshot, 0, 10.0, 0.0).is_some_and(|(x, _)| x <= bounds.max_x));
        assert!(nudge_target(&snapshot, 0, -10.0, 0.0).is_some());
    }
}
