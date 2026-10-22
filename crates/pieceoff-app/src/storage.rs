use super::*;

pub(crate) const MAX_RECENT_ROOMS: usize = 12;

pub(crate) const MAX_RECENT_ROOMS_FILE_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RecentRoom {
    pub(crate) title: String,
    pub(crate) server_url: String,
    pub(crate) invite_code: String,
    pub(crate) player_name: String,
    pub(crate) last_joined_ms: u64,
}

pub(crate) fn client_data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("PIECEOFF_CLIENT_DIR") {
        return PathBuf::from(path);
    }
    #[cfg(target_os = "windows")]
    if let Some(path) = std::env::var_os("APPDATA") {
        return PathBuf::from(path).join("PieceOff");
    }
    #[cfg(target_os = "macos")]
    if let Some(path) = std::env::var_os("HOME") {
        return PathBuf::from(path).join("Library/Application Support/PieceOff");
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(path).join("pieceoff");
    }
    if let Some(path) = std::env::var_os("HOME") {
        #[cfg(target_os = "windows")]
        return PathBuf::from(path).join("AppData/Roaming/PieceOff");
        #[cfg(target_os = "macos")]
        return PathBuf::from(path).join("Library/Application Support/PieceOff");
        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        return PathBuf::from(path).join(".local/share/pieceoff");
    }
    std::env::temp_dir().join("pieceoff-client")
}

/// Persisted per-device preferences.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub(crate) struct Settings {
    /// Faint live copy of the media behind the board. Off by default.
    pub(crate) show_ghost: bool,
    /// Freezes pan and zoom so a stray drag or wheel cannot move the board.
    pub(crate) lock_view: bool,
    /// Hide the other players' live cursors. Off by default, so they show.
    pub(crate) hide_peer_cursors: bool,
    /// Hide the solve clock in the progress readout. Off by default.
    pub(crate) hide_solve_timer: bool,
    /// Last name and server used to enter a room.
    pub(crate) player_name: Option<String>,
    pub(crate) server_url: Option<String>,
    pub(crate) audio_volume: Option<u8>,
    pub(crate) audio_muted: bool,
    /// Sound effects volume (0–100), separate from the clip's audio; clamped
    /// on load. `None` means the default.
    pub(crate) effects_volume: Option<u8>,
    pub(crate) effects_muted: bool,
    pub(crate) video_quality: VideoQuality,
    /// Delay added to the audio, in milliseconds, for slow output devices.
    pub(crate) audio_offset_ms: i32,
    /// Shortcut overrides by action id; actions not listed keep their defaults.
    pub(crate) keybindings: std::collections::BTreeMap<String, Vec<String>>,
    pub(crate) mouse: MouseSettings,
    /// A folder holding `ffmpeg` and `ffprobe`, chosen in the app.
    pub(crate) ffmpeg_dir: Option<String>,
}

pub(crate) fn settings_path() -> PathBuf {
    client_data_dir().join("settings.json")
}

pub(crate) fn load_settings(path: &Path) -> Settings {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(crate) fn save_settings(path: &Path, settings: &Settings) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(settings) {
        let _ = std::fs::write(path, bytes);
    }
}

pub(crate) fn recent_rooms_path() -> PathBuf {
    client_data_dir().join("recent-rooms.json")
}

pub(crate) fn load_recent_rooms(path: &Path) -> Result<Vec<RecentRoom>, String> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.len() > MAX_RECENT_ROOMS_FILE_BYTES {
        return Err("Recent rooms file exceeds its size limit".into());
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let mut rooms: Vec<RecentRoom> =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    rooms.retain(|room| {
        !room.title.trim().is_empty()
            && room.title.chars().count() <= 80
            && !room.title.chars().any(char::is_control)
            && !room.server_url.is_empty()
            && room.server_url.len() <= 2048
            && !room.invite_code.is_empty()
            && room.invite_code.len() <= 32
            && room
                .invite_code
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric())
            && room.player_name.chars().count() <= 32
            && !room.player_name.chars().any(char::is_control)
    });
    rooms.sort_by_key(|room| std::cmp::Reverse(room.last_joined_ms));
    rooms.truncate(MAX_RECENT_ROOMS);
    Ok(rooms)
}

pub(crate) fn save_recent_rooms(path: &Path, rooms: &[RecentRoom]) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(rooms).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_RECENT_ROOMS_FILE_BYTES {
        return Err("Recent rooms would exceed the file size limit".into());
    }
    let parent = path
        .parent()
        .ok_or("Recent rooms path has no parent directory")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let stamp = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(".recent-rooms-{}-{stamp}.tmp", std::process::id()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.write_all(&bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        // Unix rename replaces atomically. Windows requires removing the old
        // destination before renaming, but the fresh history has already been
        // flushed to its temporary file above.
        #[cfg(windows)]
        if path.exists() {
            std::fs::remove_file(path).map_err(|error| error.to_string())?;
        }
        std::fs::rename(&temporary, path).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn remember_recent_room(
    path: &Path,
    rooms: &mut Vec<RecentRoom>,
    mut recent: RecentRoom,
) -> Result<(), String> {
    recent.title = recent.title.trim().chars().take(80).collect();
    recent.server_url = recent.server_url.trim().trim_end_matches('/').to_owned();
    recent.invite_code = recent.invite_code.trim().to_ascii_uppercase();
    recent.player_name = recent.player_name.trim().chars().take(32).collect();
    if recent.server_url.is_empty()
        || recent.server_url.len() > 2048
        || recent.invite_code.is_empty()
        || recent.invite_code.len() > 32
        || !recent
            .invite_code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err("Recent room details are invalid".into());
    }
    let mut updated = rooms.clone();
    updated.retain(|room| {
        !(room.server_url.eq_ignore_ascii_case(&recent.server_url)
            && room.invite_code.eq_ignore_ascii_case(&recent.invite_code))
    });
    updated.insert(0, recent);
    updated.truncate(MAX_RECENT_ROOMS);
    save_recent_rooms(path, &updated)?;
    *rooms = updated;
    Ok(())
}

pub(crate) fn forget_recent_room(
    path: &Path,
    rooms: &mut Vec<RecentRoom>,
    index: usize,
) -> Result<(), String> {
    if index >= rooms.len() {
        return Err("Recent room selection is no longer available".into());
    }
    let mut updated = rooms.clone();
    updated.remove(index);
    save_recent_rooms(path, &updated)?;
    *rooms = updated;
    Ok(())
}
