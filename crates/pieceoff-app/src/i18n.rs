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
            // PieceOff Online.
            "Online" => "En línea",
            "Local or LAN" => "Local o red local",
            "Play on PieceOff Online with anyone, no server needed" => {
                "Juega en PieceOff Online con quien quieras, sin servidor propio"
            }
            "Use a room server on this computer or your network" => {
                "Usa un servidor de salas en este equipo o en tu red"
            }
            "Use the server on this device" => "Usar el servidor de este dispositivo",
            "This is a PieceOff Online server." => "Este es un servidor de PieceOff Online.",
            "Checking PieceOff Online…" => "Comprobando PieceOff Online…",
            "PieceOff Online is ready" => "PieceOff Online está listo",
            "PieceOff Online · this computer is linked" => {
                "PieceOff Online · este equipo está vinculado"
            }
            "Unlink" => "Desvincular",
            "Forget this computer's PieceOff Online key" => {
                "Olvidar la clave de PieceOff Online de este equipo"
            }
            "Creating your first room links this computer once, in your browser. Joining never needs it." => {
                "Al crear tu primera sala, este equipo se vincula una vez desde el navegador. Para unirse nunca hace falta."
            }
            "rooms.pieceoff.app did not answer as PieceOff Online." => {
                "rooms.pieceoff.app no respondió como PieceOff Online."
            }
            "Try again" => "Reintentar",
            "Cancel" => "Cancelar",
            "Code or invite link" => "Código o enlace de invitación",
            "Type a room code, or paste a pieceoff.app/join link" => {
                "Escribe un código de sala o pega un enlace pieceoff.app/join"
            }
            "Copy invite link" => "Copiar enlace de invitación",
            "Online rooms" => "Salas en línea",
            "Quick match" => "Partida rápida",
            "Join an open public room, or start one with a built-in puzzle" => {
                "Entra en una sala pública con sitio o abre una con un rompecabezas incluido"
            }
            "Refresh" => "Actualizar",
            "No public rooms right now. Quick match starts one." => {
                "Ahora no hay salas públicas. La partida rápida abre una."
            }
            "Watch" => "Mirar",
            "Watch without playing" => "Mirar sin jugar",
            "This room is full; you can still watch" => "La sala está llena; aún puedes mirar",
            "List publicly" => "Publicar sala",
            "Listed publicly" => "Sala pública",
            "Anyone can find this room in Online rooms and join it. Invite codes still work." => {
                "Cualquiera puede encontrar esta sala en Salas en línea y entrar. Los códigos de invitación siguen funcionando."
            }
            "Show this room in Online rooms so anyone can join. Invite codes still work." => {
                "Muestra esta sala en Salas en línea para que cualquiera pueda entrar. Los códigos de invitación siguen funcionando."
            }
            "Public rooms are only on PieceOff Online." => {
                "Las salas públicas solo existen en PieceOff Online."
            }
            "PieceOff Online has no public lobby yet." => {
                "PieceOff Online aún no tiene salas públicas."
            }
            "Public rooms use built-in puzzles only. Pick one of the built-in puzzles to list this room." => {
                "Las salas públicas solo usan rompecabezas incluidos. Elige uno de ellos para publicar esta sala."
            }
            "Public rooms use built-in puzzles only. Remove other videos from the queue to list this room." => {
                "Las salas públicas solo usan rompecabezas incluidos. Quita los otros vídeos de la cola para publicarla."
            }
            crate::online::BUILTIN_ONLY_MESSAGE => {
                "PieceOff Online usa por ahora rompecabezas incluidos. Usa Local o red local para jugar con tus archivos."
            }
            "Add built-in puzzle…" => "Añadir rompecabezas incluido…",
            "Link this computer to PieceOff Online" => "Vincular este equipo a PieceOff Online",
            "To keep rooms free of spam, each computer passes a quick check in your browser once before it creates rooms. Joining rooms never needs this." => {
                "Para evitar abusos, cada equipo pasa una vez una comprobación rápida en el navegador antes de crear salas. Para unirse nunca hace falta."
            }
            "This computer's link to PieceOff Online was revoked or has expired. Link it again to create rooms." => {
                "El vínculo de este equipo con PieceOff Online se revocó o caducó. Vuelve a vincularlo para crear salas."
            }
            "Getting a link code…" => "Obteniendo un código de vínculo…",
            "Check that the browser shows this code:" => {
                "Comprueba que el navegador muestra este código:"
            }
            "Open browser" => "Abrir el navegador",
            "Copy link" => "Copiar enlace",
            "Waiting for approval in the browser…" => "Esperando la aprobación en el navegador…",
            "That link code expired before it was approved." => {
                "El código caducó antes de aprobarse."
            }
            "Get a new code" => "Obtener otro código",
            crate::online::UNAVAILABLE_MESSAGE => {
                "PieceOff Online no está disponible ahora. Inténtalo más tarde o usa Local o red local."
            }
            crate::online::OFFLINE_MESSAGE => {
                "No se puede conectar con PieceOff Online. Revisa tu conexión a internet e inténtalo de nuevo."
            }
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
        assert_eq!(language.text("Quick match"), "Partida rápida");
        assert!(
            language
                .text(crate::online::BUILTIN_ONLY_MESSAGE)
                .starts_with("PieceOff Online usa")
        );
        assert_eq!(serde_json::to_string(&language).unwrap(), "\"spanish\"");
    }
}
