//! Configurable controls: rebindable keyboard shortcuts and mouse options,
//! persisted with the device settings.

use super::*;
use eframe::egui::RichText;
use std::collections::{BTreeMap, HashMap};

/// Something a shortcut can do. Escape (clear the selection, close a window)
/// is deliberately not here: it must always work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Action {
    FitView,
    FocusBoard,
    FocusTray,
    ZoomIn,
    ZoomOut,
    ToggleGhost,
    TidyTray,
    ShuffleTray,
    ToggleHelp,
    ToggleMessages,
    FocusChat,
    Ping,
    PlayPause,
    SeekBack,
    SeekForward,
    LoopStart,
    LoopEnd,
    NextVideo,
    PreviousVideo,
    CycleRepeat,
}

impl Action {
    pub(crate) const ALL: [Self; 20] = [
        Self::FitView,
        Self::FocusBoard,
        Self::FocusTray,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::ToggleGhost,
        Self::TidyTray,
        Self::ShuffleTray,
        Self::ToggleHelp,
        Self::ToggleMessages,
        Self::FocusChat,
        Self::Ping,
        Self::PlayPause,
        Self::SeekBack,
        Self::SeekForward,
        Self::LoopStart,
        Self::LoopEnd,
        Self::NextVideo,
        Self::PreviousVideo,
        Self::CycleRepeat,
    ];

    /// Stable name used in the settings file.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::FitView => "fit_view",
            Self::FocusBoard => "focus_board",
            Self::FocusTray => "focus_tray",
            Self::ZoomIn => "zoom_in",
            Self::ZoomOut => "zoom_out",
            Self::ToggleGhost => "toggle_ghost",
            Self::TidyTray => "tidy_tray",
            Self::ShuffleTray => "shuffle_tray",
            Self::ToggleHelp => "toggle_help",
            Self::ToggleMessages => "toggle_messages",
            Self::FocusChat => "focus_chat",
            Self::Ping => "ping",
            Self::PlayPause => "play_pause",
            Self::SeekBack => "seek_back",
            Self::SeekForward => "seek_forward",
            Self::LoopStart => "loop_start",
            Self::LoopEnd => "loop_end",
            Self::NextVideo => "next_video",
            Self::PreviousVideo => "previous_video",
            Self::CycleRepeat => "cycle_repeat",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::FitView => "Fit everything",
            Self::FocusBoard => "Focus the board",
            Self::FocusTray => "Focus the tray",
            Self::ZoomIn => "Zoom in",
            Self::ZoomOut => "Zoom out",
            Self::ToggleGhost => "Toggle the board ghost",
            Self::TidyTray => "Tidy the tray (edges first)",
            Self::ShuffleTray => "Shuffle the tray",
            Self::ToggleHelp => "Show shortcuts",
            Self::ToggleMessages => "Show message history",
            Self::FocusChat => "Type in the room chat",
            Self::Ping => "Ping the table where the pointer is",
            Self::PlayPause => "Play or pause",
            Self::SeekBack => "Seek back 5 seconds",
            Self::SeekForward => "Seek forward 5 seconds",
            Self::LoopStart => "Jump to loop start",
            Self::LoopEnd => "Jump to loop end",
            Self::NextVideo => "Next video in the queue",
            Self::PreviousVideo => "Previous video in the queue",
            Self::CycleRepeat => "Cycle repeat (off, this video, all)",
        }
    }

    pub(crate) fn host_only(self) -> bool {
        matches!(
            self,
            Self::PlayPause
                | Self::SeekBack
                | Self::SeekForward
                | Self::LoopStart
                | Self::LoopEnd
                | Self::NextVideo
                | Self::PreviousVideo
                | Self::CycleRepeat
        )
    }

    fn default_bindings(self) -> Vec<Binding> {
        use egui::Key;
        let plain = |keys: &[Key]| keys.iter().map(|key| Binding::plain(*key)).collect();
        match self {
            Self::FitView => plain(&[Key::F, Key::Num0]),
            Self::FocusBoard => plain(&[Key::B]),
            Self::FocusTray => plain(&[Key::T]),
            Self::ZoomIn => plain(&[Key::Plus, Key::Equals]),
            Self::ZoomOut => plain(&[Key::Minus]),
            Self::ToggleGhost => plain(&[Key::G]),
            // Rearranging the tray changes every player's view, so it is
            // reachable from the menu but not from a stray keypress.
            Self::TidyTray | Self::ShuffleTray => Vec::new(),
            // `?` needs Shift on most layouts; a binding ignores extra Shift.
            Self::ToggleHelp => plain(&[Key::F1, Key::Questionmark]),
            Self::ToggleMessages => plain(&[Key::M]),
            Self::FocusChat => plain(&[Key::Enter]),
            // A key rather than a click, so it never competes with the
            // mouse options (select modifier, pan button).
            Self::Ping => plain(&[Key::P]),
            Self::PlayPause => plain(&[Key::Space]),
            Self::SeekBack => plain(&[Key::ArrowLeft]),
            Self::SeekForward => plain(&[Key::ArrowRight]),
            Self::LoopStart => plain(&[Key::Home]),
            Self::LoopEnd => plain(&[Key::End]),
            Self::NextVideo => plain(&[Key::PageDown]),
            Self::PreviousVideo => plain(&[Key::PageUp]),
            Self::CycleRepeat => plain(&[Key::R]),
        }
    }
}

/// A key plus the modifiers that must be held with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) key: egui::Key,
    /// Ctrl (Cmd on macOS).
    pub(crate) command: bool,
    pub(crate) shift: bool,
    pub(crate) alt: bool,
}

impl Binding {
    pub(crate) fn plain(key: egui::Key) -> Self {
        Self {
            key,
            command: false,
            shift: false,
            alt: false,
        }
    }

    pub(crate) fn from_event(key: egui::Key, modifiers: egui::Modifiers) -> Self {
        Self {
            key,
            command: modifiers.command,
            shift: modifiers.shift,
            alt: modifiers.alt,
        }
    }

    pub(crate) fn modifiers(self) -> egui::Modifiers {
        let mut modifiers = egui::Modifiers::NONE;
        if self.command {
            modifiers |= egui::Modifiers::COMMAND;
        }
        if self.shift {
            modifiers |= egui::Modifiers::SHIFT;
        }
        if self.alt {
            modifiers |= egui::Modifiers::ALT;
        }
        modifiers
    }

    /// The form written to the settings file, such as `Ctrl+Shift+T`.
    pub(crate) fn to_setting(self) -> String {
        let mut text = String::new();
        if self.command {
            text.push_str("Ctrl+");
        }
        if self.shift {
            text.push_str("Shift+");
        }
        if self.alt {
            text.push_str("Alt+");
        }
        text.push_str(self.key.name());
        text
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        let mut binding = Self::plain(egui::Key::A);
        let mut parts: Vec<&str> = text.trim().split('+').collect();
        let key = parts.pop()?;
        binding.key = egui::Key::from_name(key.trim())?;
        // The name must round-trip, or the setting would silently change.
        if binding.key.name() != key.trim() {
            return None;
        }
        for modifier in parts {
            match modifier.trim() {
                "Ctrl" => binding.command = true,
                "Shift" => binding.shift = true,
                "Alt" => binding.alt = true,
                _ => return None,
            }
        }
        Some(binding)
    }

    /// What the player sees; Ctrl reads as Cmd on macOS.
    pub(crate) fn label(self) -> String {
        let command = if cfg!(target_os = "macos") {
            "Cmd+"
        } else {
            "Ctrl+"
        };
        let mut text = String::new();
        if self.command {
            text.push_str(command);
        }
        if self.shift {
            text.push_str("Shift+");
        }
        if self.alt {
            text.push_str("Alt+");
        }
        text.push_str(match self.key {
            egui::Key::Plus => "+",
            egui::Key::Minus => "-",
            egui::Key::Equals => "=",
            egui::Key::Questionmark => "?",
            egui::Key::Num0 => "0",
            key => key.name(),
        });
        text
    }

    /// Two bindings clash when one would also fire for the other's
    /// keypress: the same key with nested modifier sets.
    pub(crate) fn conflicts_with(self, other: Self) -> bool {
        self.key == other.key
            && ((!self.command || other.command)
                && (!self.shift || other.shift)
                && (!self.alt || other.alt)
                || (!other.command || self.command)
                    && (!other.shift || self.shift)
                    && (!other.alt || self.alt))
    }
}

/// The active key map.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Controls {
    bindings: HashMap<Action, Vec<Binding>>,
}

impl Default for Controls {
    fn default() -> Self {
        Self::from_settings(&BTreeMap::new())
    }
}

impl Controls {
    /// Apply saved overrides to the defaults. An action missing from the
    /// file keeps its defaults; an empty list means it was unbound on
    /// purpose; a list with only unreadable entries falls back to defaults.
    pub(crate) fn from_settings(saved: &BTreeMap<String, Vec<String>>) -> Self {
        let mut bindings = HashMap::new();
        for action in Action::ALL {
            let configured = match saved.get(action.id()) {
                None => action.default_bindings(),
                Some(entries) if entries.is_empty() => Vec::new(),
                Some(entries) => {
                    let parsed: Vec<Binding> =
                        entries.iter().filter_map(|e| Binding::parse(e)).collect();
                    if parsed.is_empty() {
                        action.default_bindings()
                    } else {
                        parsed
                    }
                }
            };
            bindings.insert(action, configured);
        }
        Self { bindings }
    }

    /// Only what differs from the defaults, so new defaults reach players
    /// who never customised that action.
    pub(crate) fn to_settings(&self) -> BTreeMap<String, Vec<String>> {
        Action::ALL
            .into_iter()
            .filter(|action| self.bindings_for(*action) != action.default_bindings())
            .map(|action| {
                (
                    action.id().to_owned(),
                    self.bindings_for(action)
                        .iter()
                        .map(|binding| binding.to_setting())
                        .collect(),
                )
            })
            .collect()
    }

    pub(crate) fn bindings_for(&self, action: Action) -> &[Binding] {
        self.bindings.get(&action).map_or(&[], Vec::as_slice)
    }

    /// The action already using `binding`, other than `except`.
    pub(crate) fn conflict(&self, binding: Binding, except: Action) -> Option<Action> {
        Action::ALL.into_iter().find(|action| {
            *action != except
                && self
                    .bindings_for(*action)
                    .iter()
                    .any(|existing| existing.conflicts_with(binding))
        })
    }

    pub(crate) fn set(&mut self, action: Action, bindings: Vec<Binding>) {
        self.bindings.insert(action, bindings);
    }

    pub(crate) fn reset(&mut self, action: Action) {
        self.bindings.insert(action, action.default_bindings());
    }

    /// Consume the keypress for `action`, if one is pending.
    pub(crate) fn pressed(&self, ctx: &egui::Context, action: Action) -> bool {
        ctx.input_mut(|input| {
            let mut fired = false;
            for binding in self.bindings_for(action) {
                // Consume every match so a held key cannot leak through.
                fired |= input.consume_key(binding.modifiers(), binding.key);
            }
            fired
        })
    }

    /// The bindings as one line, such as `F, 0`.
    pub(crate) fn describe(&self, action: Action) -> String {
        let bindings = self.bindings_for(action);
        if bindings.is_empty() {
            return "Not bound".into();
        }
        let mut labels: Vec<String> = Vec::new();
        for binding in bindings {
            let label = binding.label();
            // `Shift+?` and `?` read as one shortcut.
            if !labels.contains(&label) {
                labels.push(label);
            }
        }
        labels.join(", ")
    }
}

/// Which modifier turns a click or drag into a multi-selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SelectModifier {
    #[default]
    Command,
    Alt,
}

impl SelectModifier {
    pub(crate) fn label(self) -> &'static str {
        match (self, cfg!(target_os = "macos")) {
            (Self::Command, true) => "Cmd",
            (Self::Command, false) => "Ctrl",
            (Self::Alt, _) => "Alt",
        }
    }

    pub(crate) fn held(self, modifiers: &egui::Modifiers) -> bool {
        match self {
            Self::Command => modifiers.command,
            Self::Alt => modifiers.alt,
        }
    }
}

/// An extra mouse button that pans the board while held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PanButton {
    Off,
    #[default]
    Middle,
    Right,
}

impl PanButton {
    pub(crate) fn pointer_button(self) -> Option<egui::PointerButton> {
        match self {
            Self::Off => None,
            Self::Middle => Some(egui::PointerButton::Middle),
            Self::Right => Some(egui::PointerButton::Secondary),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Middle => "Middle button",
            Self::Right => "Right button",
        }
    }
}

pub(crate) const MIN_WHEEL_SPEED: f32 = 0.25;
pub(crate) const MAX_WHEEL_SPEED: f32 = 3.0;
pub(crate) const MIN_ZOOM_STEP: f32 = 1.1;
pub(crate) const MAX_ZOOM_STEP: f32 = 2.0;
pub(crate) const MAX_GRAB_MARGIN: f32 = 40.0;

/// Mouse preferences, saved with the device settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub(crate) struct MouseSettings {
    /// Multiplier on how far one wheel notch zooms.
    pub(crate) wheel_speed: f32,
    pub(crate) invert_wheel: bool,
    /// Factor applied by the zoom buttons and keys.
    pub(crate) zoom_step: f32,
    pub(crate) select_modifier: SelectModifier,
    /// Dragging empty board space pans (Shift-drag always does).
    pub(crate) drag_empty_pans: bool,
    pub(crate) pan_button: PanButton,
    /// Screen points beyond a piece's outline that still grab it.
    pub(crate) grab_margin: f32,
}

impl Default for MouseSettings {
    fn default() -> Self {
        Self {
            wheel_speed: 1.0,
            invert_wheel: false,
            zoom_step: 1.25,
            select_modifier: SelectModifier::Command,
            drag_empty_pans: true,
            pan_button: PanButton::Middle,
            grab_margin: 10.0,
        }
    }
}

impl MouseSettings {
    /// Settings read from disk can be anything; keep them usable.
    pub(crate) fn sanitized(mut self) -> Self {
        let finite_or =
            |value: f32, fallback: f32| if value.is_finite() { value } else { fallback };
        self.wheel_speed = finite_or(self.wheel_speed, 1.0).clamp(MIN_WHEEL_SPEED, MAX_WHEEL_SPEED);
        self.zoom_step = finite_or(self.zoom_step, 1.25).clamp(MIN_ZOOM_STEP, MAX_ZOOM_STEP);
        self.grab_margin = finite_or(self.grab_margin, 10.0).clamp(0.0, MAX_GRAB_MARGIN);
        self
    }

    /// A wheel movement in the direction and strength the player chose.
    pub(crate) fn adjust_scroll(&self, scroll_y: f32) -> f32 {
        let direction = if self.invert_wheel { -1.0 } else { 1.0 };
        scroll_y * self.wheel_speed * direction
    }
}

impl PieceOffApp {
    /// Persist the key map and mouse options after an edit.
    pub(crate) fn apply_controls(&mut self, controls: Controls) {
        self.settings.keybindings = controls.to_settings();
        self.controls = controls;
        self.persist_settings();
    }

    /// While a row is waiting for a key, take the next keypress for it.
    fn capture_rebinding(&mut self, ctx: &egui::Context) {
        let Some(action) = self.rebinding else { return };
        let pressed = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } => Some((*key, *modifiers)),
                _ => None,
            })
        });
        let Some((key, modifiers)) = pressed else {
            return;
        };
        if key == egui::Key::Escape {
            self.rebinding = None;
            self.controls_notice = None;
            return;
        }
        let binding = Binding::from_event(key, modifiers);
        if let Some(other) = self.controls.conflict(binding, action) {
            self.controls_notice = Some(format!(
                "{} is already used by “{}”. Pick another key.",
                binding.label(),
                other.label()
            ));
            return;
        }
        let mut controls = self.controls.clone();
        controls.set(action, vec![binding]);
        self.apply_controls(controls);
        self.rebinding = None;
        self.controls_notice = None;
        self.status = format!("{} is now {}.", action.label(), binding.label());
    }

    pub(crate) fn controls_window(&mut self, ctx: &egui::Context) {
        self.capture_rebinding(ctx);
        let mut open = true;
        let mut controls = self.controls.clone();
        let mut mouse = self.settings.mouse.clone();
        let mut start_rebinding = None;
        let mut reset_all = false;
        egui::Window::new("Controls")
            .open(&mut open)
            .collapsible(false)
            .default_width(580.0)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(520.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Keyboard").strong());
                        ui.weak("Click Change, then press the new key. Esc cancels.");
                        if let Some(notice) = &self.controls_notice {
                            ui.colored_label(Color32::from_rgb(240, 110, 110), notice);
                        }
                        ui.add_space(4.0);
                        egui::Grid::new("key-bindings")
                            .num_columns(3)
                            .spacing([16.0, 6.0])
                            .show(ui, |ui| {
                                for action in Action::ALL {
                                    let label = if action.host_only() {
                                        format!("{} (host)", action.label())
                                    } else {
                                        action.label().to_owned()
                                    };
                                    ui.label(label);
                                    if self.rebinding == Some(action) {
                                        ui.colored_label(ACCENT, "Press a key…");
                                    } else {
                                        ui.label(
                                            RichText::new(controls.describe(action)).monospace(),
                                        );
                                    }
                                    ui.horizontal(|ui| {
                                        if ui.small_button("Change").clicked() {
                                            start_rebinding = Some(action);
                                        }
                                        let defaults_differ = controls.bindings_for(action)
                                            != action.default_bindings();
                                        if ui
                                            .add_enabled(
                                                defaults_differ,
                                                egui::Button::new("Reset").small(),
                                            )
                                            .clicked()
                                        {
                                            controls.reset(action);
                                        }
                                        if ui
                                            .add_enabled(
                                                !controls.bindings_for(action).is_empty(),
                                                egui::Button::new("Unbind").small(),
                                            )
                                            .clicked()
                                        {
                                            controls.set(action, Vec::new());
                                        }
                                    });
                                    ui.end_row();
                                }
                            });
                        ui.add_space(10.0);
                        ui.label(RichText::new("Mouse").strong());
                        egui::Grid::new("mouse-options")
                            .num_columns(2)
                            .spacing([16.0, 6.0])
                            .show(ui, |ui| {
                                ui.label("Wheel zoom speed");
                                ui.add(
                                    egui::Slider::new(
                                        &mut mouse.wheel_speed,
                                        MIN_WHEEL_SPEED..=MAX_WHEEL_SPEED,
                                    )
                                    .suffix("×"),
                                );
                                ui.end_row();
                                ui.label("Zoom button step");
                                ui.add(
                                    egui::Slider::new(
                                        &mut mouse.zoom_step,
                                        MIN_ZOOM_STEP..=MAX_ZOOM_STEP,
                                    )
                                    .suffix("×"),
                                );
                                ui.end_row();
                                ui.label("Invert the wheel");
                                ui.checkbox(&mut mouse.invert_wheel, "Scroll up zooms out");
                                ui.end_row();
                                ui.label("Multi-select modifier");
                                egui::ComboBox::from_id_salt("select-modifier")
                                    .selected_text(format!(
                                        "{}-click",
                                        mouse.select_modifier.label()
                                    ))
                                    .show_ui(ui, |ui| {
                                        for modifier in
                                            [SelectModifier::Command, SelectModifier::Alt]
                                        {
                                            ui.selectable_value(
                                                &mut mouse.select_modifier,
                                                modifier,
                                                format!("{}-click", modifier.label()),
                                            );
                                        }
                                    });
                                ui.end_row();
                                ui.label("Piece grab margin");
                                ui.add(
                                    egui::Slider::new(
                                        &mut mouse.grab_margin,
                                        0.0..=MAX_GRAB_MARGIN,
                                    )
                                    .suffix(" px"),
                                )
                                .on_hover_text("How far outside a piece's edge still picks it up");
                                ui.end_row();
                                ui.label("Drag empty space");
                                ui.checkbox(&mut mouse.drag_empty_pans, "Pans the board");
                                ui.end_row();
                                ui.label("Pan with button");
                                egui::ComboBox::from_id_salt("pan-button")
                                    .selected_text(mouse.pan_button.label())
                                    .show_ui(ui, |ui| {
                                        for button in
                                            [PanButton::Middle, PanButton::Right, PanButton::Off]
                                        {
                                            ui.selectable_value(
                                                &mut mouse.pan_button,
                                                button,
                                                button.label(),
                                            );
                                        }
                                    });
                                ui.end_row();
                            });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Reset everything").clicked() {
                                reset_all = true;
                            }
                            ui.weak("Shift-drag always pans. Esc always clears and closes.");
                        });
                    });
            });
        if reset_all {
            controls = Controls::default();
            mouse = MouseSettings::default();
            self.rebinding = None;
            self.controls_notice = None;
        }
        if let Some(action) = start_rebinding {
            self.rebinding = Some(action);
            self.controls_notice = None;
        }
        let mouse = mouse.sanitized();
        if controls != self.controls {
            self.apply_controls(controls);
        }
        if mouse != self.settings.mouse {
            self.settings.mouse = mouse;
            self.persist_settings();
        }
        if !open {
            self.rebinding = None;
            self.controls_notice = None;
        }
        self.controls_open = open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bindings_never_clash_between_actions() {
        let controls = Controls::default();
        for first in Action::ALL {
            for second in Action::ALL {
                if first == second {
                    continue;
                }
                for a in controls.bindings_for(first) {
                    for b in controls.bindings_for(second) {
                        assert!(
                            !a.conflicts_with(*b),
                            "{} and {} both use {}",
                            first.label(),
                            second.label(),
                            a.label()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_default_binding_round_trips_through_its_setting_text() {
        for action in Action::ALL {
            for binding in action.default_bindings() {
                let text = binding.to_setting();
                assert_eq!(Binding::parse(&text), Some(binding), "{text}");
            }
        }
        let combo = Binding {
            key: egui::Key::T,
            command: true,
            shift: true,
            alt: true,
        };
        assert_eq!(combo.to_setting(), "Ctrl+Shift+Alt+T");
        assert_eq!(Binding::parse("Ctrl+Shift+Alt+T"), Some(combo));
        assert_eq!(Binding::parse("Hyper+T"), None);
        assert_eq!(Binding::parse("Ctrl+"), None);
        assert_eq!(Binding::parse(""), None);
    }

    #[test]
    fn only_customised_actions_are_saved_and_they_reload_identically() {
        let mut controls = Controls::default();
        assert!(controls.to_settings().is_empty());
        controls.set(Action::FocusTray, vec![Binding::plain(egui::Key::K)]);
        controls.set(Action::ToggleGhost, Vec::new());
        controls.set(
            Action::TidyTray,
            vec![Binding {
                command: true,
                ..Binding::plain(egui::Key::E)
            }],
        );
        let saved = controls.to_settings();
        assert_eq!(saved.len(), 3);
        assert_eq!(saved["toggle_ghost"], Vec::<String>::new());
        let json = serde_json::to_string(&saved).unwrap();
        let reloaded = Controls::from_settings(&serde_json::from_str(&json).unwrap());
        assert_eq!(reloaded, controls);
    }

    #[test]
    fn unreadable_saved_bindings_fall_back_to_the_defaults() {
        let saved = BTreeMap::from([
            (
                "focus_tray".to_owned(),
                vec!["Bogus+Q".to_owned(), "".to_owned()],
            ),
            ("no_such_action".to_owned(), vec!["Q".to_owned()]),
            (
                "fit_view".to_owned(),
                vec!["Nonsense".to_owned(), "H".to_owned()],
            ),
        ]);
        let controls = Controls::from_settings(&saved);
        assert_eq!(
            controls.bindings_for(Action::FocusTray),
            Action::FocusTray.default_bindings()
        );
        // A partly valid list keeps what it can read.
        assert_eq!(
            controls.bindings_for(Action::FitView),
            [Binding::plain(egui::Key::H)]
        );
    }

    #[test]
    fn conflicts_see_nested_modifiers_but_not_different_keys() {
        let t = Binding::plain(egui::Key::T);
        let shift_t = Binding { shift: true, ..t };
        let ctrl_t = Binding { command: true, ..t };
        assert!(t.conflicts_with(shift_t), "Shift+T also fires plain T");
        assert!(t.conflicts_with(ctrl_t));
        assert!(
            !shift_t.conflicts_with(ctrl_t),
            "neither contains the other"
        );
        assert!(!t.conflicts_with(Binding::plain(egui::Key::Y)));

        let controls = Controls::default();
        assert_eq!(
            controls.conflict(Binding::plain(egui::Key::B), Action::FitView),
            Some(Action::FocusBoard)
        );
        assert_eq!(
            controls.conflict(Binding::plain(egui::Key::B), Action::FocusBoard),
            None
        );
    }

    #[test]
    fn mouse_settings_are_clamped_and_old_files_get_defaults() {
        let wild = MouseSettings {
            wheel_speed: 90.0,
            zoom_step: f32::NAN,
            ..MouseSettings::default()
        }
        .sanitized();
        assert_eq!(wild.wheel_speed, MAX_WHEEL_SPEED);
        assert_eq!(wild.zoom_step, 1.25);

        let partial: MouseSettings = serde_json::from_str(r#"{"invert_wheel": true}"#).unwrap();
        assert!(partial.invert_wheel);
        assert_eq!(partial.wheel_speed, 1.0);
        assert!(partial.drag_empty_pans);

        let inverted = MouseSettings {
            invert_wheel: true,
            wheel_speed: 2.0,
            ..MouseSettings::default()
        };
        assert_eq!(inverted.adjust_scroll(10.0), -20.0);
    }
}
