use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Language {
    #[default]
    English,
    Spanish,
}

impl Language {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Spanish => "Español",
        }
    }

    /// UI copy translated independently of wire messages and saved puzzle data.
    /// Untranslated keys fall back to English while the catalog grows.
    pub(crate) fn text(self, english: &'static str) -> &'static str {
        if self == Self::English {
            return english;
        }
        match english {
            "Language" => "Idioma",
            "Interface size" => "Tamaño de la interfaz",
            "Start a puzzle" => "Empezar un rompecabezas",
            "Choose media…" => "Elegir archivo…",
            "Open package…" => "Abrir paquete…",
            "Back to library" => "Volver a la biblioteca",
            "Pick an image or video, or drop one on the window." => {
                "Elige una imagen o un vídeo, o arrástralo a la ventana."
            }
            "Title" => "Título",
            "Cut" => "Corte",
            "Pieces" => "Piezas",
            "This package keeps its saved pieces and progress." => {
                "Este paquete conserva las piezas y el progreso guardados."
            }
            "Create room from package" => "Crear sala desde el paquete",
            "Create room" => "Crear sala",
            "Join a room" => "Unirse a una sala",
            "Invite code" => "Código de invitación",
            "Join" => "Unirse",
            "Watch only" => "Solo mirar",
            "Recent rooms" => "Salas recientes",
            "Forget" => "Olvidar",
            "You" => "Tú",
            "Name" => "Nombre",
            "Controls…" => "Controles…",
            "Room server" => "Servidor de salas",
            "Reset" => "Restablecer",
            "More" => "Más",
            "Leave" => "Salir",
            "Rejoin" => "Volver a entrar",
            "Watching" => "Mirando",
            "Connecting…" => "Conectando…",
            "Offline" => "Sin conexión",
            "Uploading media…" => "Subiendo archivo…",
            "Media only on this device" => "Archivo solo en este dispositivo",
            "Fit" => "Ajustar",
            "Board" => "Tablero",
            "Tray" => "Bandeja",
            "Lock" => "Bloquear",
            "Arrange" => "Ordenar",
            "Holders" => "Soportes",
            "Reference image" => "Imagen de referencia",
            "Watch timelapse" => "Ver secuencia rápida",
            "Save package…" => "Guardar paquete…",
            "Save to library" => "Guardar en biblioteca",
            "Update library copy" => "Actualizar copia en biblioteca",
            "Restart video" => "Reiniciar vídeo",
            "Keyboard play…" => "Jugar con teclado…",
            "Keyboard play" => "Juego con teclado",
            "Use Tab and Enter to choose a piece and move it. The room server decides when it snaps into place." => {
                "Usa Tab e Intro para elegir y mover una pieza. El servidor decide cuándo encaja."
            }
            "No loose pieces are available." => "No hay piezas sueltas disponibles.",
            "Previous piece" => "Pieza anterior",
            "Next piece" => "Pieza siguiente",
            "Move up" => "Mover arriba",
            "Move down" => "Mover abajo",
            "Move left" => "Mover a la izquierda",
            "Move right" => "Mover a la derecha",
            _ => english,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_round_trips_and_unknown_copy_falls_back() {
        let language: Language = serde_json::from_str("\"spanish\"").unwrap();
        assert_eq!(language.text("Create room"), "Crear sala");
        assert_eq!(language.text("Untranslated"), "Untranslated");
        assert_eq!(serde_json::to_string(&language).unwrap(), "\"spanish\"");
    }
}
