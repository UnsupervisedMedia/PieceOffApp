//! Finding FFmpeg and FFprobe, and choosing what to encode clips with.
//!
//! PieceOff runs FFmpeg as a separate program. Release bundles ship an
//! LGPL build beside the app, so a player needs nothing installed; anyone can
//! still point the app at their own build.

use super::*;
use std::collections::HashSet;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tool {
    Ffmpeg,
    Ffprobe,
}

impl Tool {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg",
            Self::Ffprobe => "ffprobe",
        }
    }

    fn env_file(self) -> &'static str {
        match self {
            Self::Ffmpeg => "PIECEOFF_FFMPEG",
            Self::Ffprobe => "PIECEOFF_FFPROBE",
        }
    }

    fn file_name(self) -> String {
        format!("{}{}", self.name(), std::env::consts::EXE_SUFFIX)
    }
}

/// Where a program was found, in the order the search tries them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolSource {
    /// A folder the player picked in the app.
    Chosen,
    /// `PIECEOFF_FFMPEG_DIR`, `PIECEOFF_FFMPEG`, or `PIECEOFF_FFPROBE`.
    Environment,
    /// Shipped beside the app.
    Bundled,
    /// Found on `PATH`.
    System,
    /// Not found; the bare name is returned so starting it reports the error.
    Missing,
}

impl ToolSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Chosen => "chosen folder",
            Self::Environment => "environment",
            Self::Bundled => "bundled with PieceOff",
            Self::System => "system",
            Self::Missing => "not found",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedTool {
    pub(crate) program: OsString,
    pub(crate) source: ToolSource,
}

/// Everything the search depends on, so it can be tested without touching
/// the real environment.
#[derive(Clone, Debug, Default)]
pub(crate) struct Search {
    pub(crate) chosen_dir: Option<PathBuf>,
    pub(crate) env_dir: Option<PathBuf>,
    pub(crate) env_ffmpeg: Option<PathBuf>,
    pub(crate) env_ffprobe: Option<PathBuf>,
    /// The folder holding the running app.
    pub(crate) app_dir: Option<PathBuf>,
    pub(crate) path_dirs: Vec<PathBuf>,
}

static CHOSEN_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Remember the folder the player chose (or forget it with `None`).
pub(crate) fn set_chosen_directory(dir: Option<PathBuf>) {
    *CHOSEN_DIR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = dir;
}

impl Search {
    pub(crate) fn current() -> Self {
        let env_path = |name: &str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Self {
            chosen_dir: CHOSEN_DIR
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
            env_dir: env_path("PIECEOFF_FFMPEG_DIR"),
            env_ffmpeg: env_path(Tool::Ffmpeg.env_file()),
            env_ffprobe: env_path(Tool::Ffprobe.env_file()),
            app_dir: std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf)),
            path_dirs: std::env::var_os("PATH")
                .map(|paths| std::env::split_paths(&paths).collect())
                .unwrap_or_default(),
        }
    }

    /// Folders a release bundle may keep its tools in: beside the app, in an
    /// `ffmpeg` or `bin` folder there, or in a macOS app bundle's Resources.
    fn bundled_dirs(&self) -> Vec<PathBuf> {
        let Some(app) = &self.app_dir else {
            return Vec::new();
        };
        let mut dirs = vec![app.clone(), app.join("ffmpeg"), app.join("bin")];
        if let Some(contents) = app.parent() {
            dirs.push(contents.join("Resources"));
            dirs.push(contents.join("Resources").join("ffmpeg"));
        }
        dirs
    }

    pub(crate) fn resolve(&self, tool: Tool) -> ResolvedTool {
        let file = tool.file_name();
        let found = |source, path: PathBuf| {
            Some(ResolvedTool {
                program: path.into_os_string(),
                source,
            })
        };
        let in_dir = |dir: &Path| Some(dir.join(&file)).filter(|path| path.is_file());
        let env_file = match tool {
            Tool::Ffmpeg => &self.env_ffmpeg,
            Tool::Ffprobe => &self.env_ffprobe,
        };
        self.chosen_dir
            .as_deref()
            .and_then(in_dir)
            .and_then(|path| found(ToolSource::Chosen, path))
            .or_else(|| {
                env_file
                    .clone()
                    .filter(|path| path.is_file())
                    .and_then(|path| found(ToolSource::Environment, path))
            })
            .or_else(|| {
                self.env_dir
                    .as_deref()
                    .and_then(in_dir)
                    .and_then(|path| found(ToolSource::Environment, path))
            })
            .or_else(|| {
                self.bundled_dirs()
                    .iter()
                    .find_map(|dir| in_dir(dir))
                    .and_then(|path| found(ToolSource::Bundled, path))
            })
            .or_else(|| {
                self.path_dirs
                    .iter()
                    .find_map(|dir| in_dir(dir))
                    .and_then(|path| found(ToolSource::System, path))
            })
            .unwrap_or(ResolvedTool {
                program: OsString::from(tool.name()),
                source: ToolSource::Missing,
            })
    }
}

pub(crate) fn resolve(tool: Tool) -> ResolvedTool {
    Search::current().resolve(tool)
}

pub(crate) fn ffmpeg() -> OsString {
    resolve(Tool::Ffmpeg).program
}

pub(crate) fn ffprobe() -> OsString {
    resolve(Tool::Ffprobe).program
}

/// What the app found, for display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolReport {
    pub(crate) tool: Tool,
    pub(crate) resolved: ResolvedTool,
    /// The first line of `-version`, when the program runs.
    pub(crate) version: Option<String>,
}

pub(crate) fn report(tool: Tool) -> ToolReport {
    let resolved = resolve(tool);
    let version = Command::new(&resolved.program)
        .arg("-version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                // "ffmpeg version n8.1 Copyright (c) 2000-2026 …": the
                // copyright notice only makes the line too long to show.
                .map(|line| {
                    line.split(" Copyright")
                        .next()
                        .unwrap_or(line)
                        .trim()
                        .to_owned()
                })
        });
    ToolReport {
        tool,
        resolved,
        version,
    }
}

/// How a clip's video is encoded. Builds that include libx264 are GPL;
/// the bundled LGPL build has OpenH264 and the native MPEG-4 encoder, so
/// clip cutting works with whatever FFmpeg is present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VideoEncoder {
    X264,
    OpenH264,
    Mpeg4,
}

impl VideoEncoder {
    const PREFERENCE: [Self; 3] = [Self::X264, Self::OpenH264, Self::Mpeg4];

    fn name(self) -> &'static str {
        match self {
            Self::X264 => "libx264",
            Self::OpenH264 => "libopenh264",
            Self::Mpeg4 => "mpeg4",
        }
    }

    /// The encoder options (after the input and filters) for an MP4 clip.
    pub(crate) fn args(self) -> Vec<&'static str> {
        let mut args = vec!["-c:v", self.name()];
        args.extend(match self {
            Self::X264 => ["-preset", "veryfast", "-crf", "23"],
            Self::OpenH264 => ["-b:v", "4M", "-rc_mode", "quality"],
            Self::Mpeg4 => ["-q:v", "4", "-qmin", "2"],
        });
        args.extend(["-pix_fmt", "yuv420p", "-movflags", "+faststart"]);
        args
    }

    /// The best encoder among `available` names, or the native one, which
    /// every build has.
    pub(crate) fn best_of(available: &HashSet<String>) -> Self {
        Self::PREFERENCE
            .into_iter()
            .find(|encoder| available.contains(encoder.name()))
            .unwrap_or(Self::Mpeg4)
    }

    /// Ask `program` which video encoders it has.
    pub(crate) fn choose(program: &OsStr) -> Self {
        let listing = Command::new(program)
            .args(["-hide_banner", "-v", "error", "-encoders"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_default();
        Self::best_of(&parse_encoder_names(&listing))
    }
}

/// Encoder names from `ffmpeg -encoders`: lines look like
/// ` V....D libx264   H.264 / AVC`, after a header ending in `------`.
pub(crate) fn parse_encoder_names(listing: &str) -> HashSet<String> {
    let mut lines = listing.lines();
    let mut names = HashSet::new();
    for line in lines.by_ref() {
        if line.trim_start().starts_with("------") {
            break;
        }
    }
    for line in lines {
        let mut fields = line.split_whitespace();
        if let (Some(flags), Some(name)) = (fields.next(), fields.next()) {
            if flags.len() == 6 {
                names.insert(name.to_owned());
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pieceoff-tools-test-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(dir: &Path, tool: Tool) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(tool.file_name());
        std::fs::write(&path, b"").unwrap();
        path
    }

    #[test]
    fn the_search_prefers_a_chosen_folder_then_the_environment_then_the_bundle_then_path() {
        let root = scratch("order");
        let chosen = root.join("chosen");
        let env_dir = root.join("env");
        let app = root.join("app");
        let system = root.join("system");
        for dir in [&chosen, &env_dir, &app, &system] {
            touch(dir, Tool::Ffmpeg);
        }
        let mut search = Search {
            chosen_dir: Some(chosen.clone()),
            env_dir: Some(env_dir.clone()),
            app_dir: Some(app.clone()),
            path_dirs: vec![system.clone()],
            ..Search::default()
        };
        let expect = |search: &Search, dir: &Path, source| {
            assert_eq!(
                search.resolve(Tool::Ffmpeg),
                ResolvedTool {
                    program: dir.join(Tool::Ffmpeg.file_name()).into_os_string(),
                    source
                }
            );
        };
        expect(&search, &chosen, ToolSource::Chosen);
        search.chosen_dir = None;
        expect(&search, &env_dir, ToolSource::Environment);
        search.env_dir = None;
        expect(&search, &app, ToolSource::Bundled);
        search.app_dir = None;
        expect(&search, &system, ToolSource::System);
        search.path_dirs.clear();
        assert_eq!(
            search.resolve(Tool::Ffmpeg),
            ResolvedTool {
                program: OsString::from("ffmpeg"),
                source: ToolSource::Missing
            }
        );
    }

    #[test]
    fn a_specific_environment_file_beats_the_environment_folder_and_each_tool_is_separate() {
        let root = scratch("env");
        let dir = root.join("dir");
        touch(&dir, Tool::Ffmpeg);
        touch(&dir, Tool::Ffprobe);
        let special = touch(&root.join("special"), Tool::Ffmpeg);
        let search = Search {
            env_dir: Some(dir.clone()),
            env_ffmpeg: Some(special.clone()),
            ..Search::default()
        };
        assert_eq!(
            search.resolve(Tool::Ffmpeg).program,
            special.into_os_string()
        );
        // Only ffmpeg was overridden; ffprobe still comes from the folder.
        assert_eq!(
            search.resolve(Tool::Ffprobe).program,
            dir.join(Tool::Ffprobe.file_name()).into_os_string()
        );
        // A path that does not exist is ignored rather than trusted.
        let search = Search {
            env_ffmpeg: Some(root.join("nothing-here")),
            ..Search::default()
        };
        assert_eq!(search.resolve(Tool::Ffmpeg).source, ToolSource::Missing);
    }

    #[test]
    fn bundles_may_keep_tools_in_a_subfolder_or_a_macos_resources_folder() {
        let root = scratch("layouts");
        let app = root.join("PieceOff.app/Contents/MacOS");
        let in_resources = touch(
            &root.join("PieceOff.app/Contents/Resources/ffmpeg"),
            Tool::Ffmpeg,
        );
        let search = Search {
            app_dir: Some(app.clone()),
            ..Search::default()
        };
        // Compared as paths: Windows joins this with mixed separators.
        assert_eq!(
            PathBuf::from(search.resolve(Tool::Ffmpeg).program),
            in_resources
        );

        let beside = root.join("beside");
        let in_subfolder = touch(&beside.join("ffmpeg"), Tool::Ffprobe);
        let search = Search {
            app_dir: Some(beside),
            ..Search::default()
        };
        let found = search.resolve(Tool::Ffprobe);
        assert_eq!(PathBuf::from(found.program), in_subfolder);
        assert_eq!(found.source, ToolSource::Bundled);
    }

    #[test]
    fn encoders_are_read_from_the_listing_and_the_best_available_wins() {
        let listing = " Encoders:\n V..... = Video\n A..... = Audio\n ------\n \
             V....D libopenh264          OpenH264 H.264 / AVC\n \
             V.S..D mpeg4                MPEG-4 part 2\n \
             A....D aac                  AAC (Advanced Audio Coding)\n";
        let names = parse_encoder_names(listing);
        assert!(names.contains("libopenh264") && names.contains("mpeg4") && names.contains("aac"));
        assert!(!names.contains("Video"), "legend lines are not encoders");
        assert_eq!(VideoEncoder::best_of(&names), VideoEncoder::OpenH264);

        let mut with_x264 = names.clone();
        with_x264.insert("libx264".into());
        assert_eq!(VideoEncoder::best_of(&with_x264), VideoEncoder::X264);

        // A build with neither falls back to the encoder every build has.
        assert_eq!(VideoEncoder::best_of(&HashSet::new()), VideoEncoder::Mpeg4);
        // And a missing program does the same rather than failing.
        assert_eq!(
            VideoEncoder::choose(OsStr::new("/definitely/not/ffmpeg")),
            VideoEncoder::Mpeg4
        );
    }

    #[test]
    fn every_encoder_writes_a_web_friendly_mp4() {
        for encoder in VideoEncoder::PREFERENCE {
            let args = encoder.args();
            assert_eq!(args[..2], ["-c:v", encoder.name()]);
            assert!(args.windows(2).any(|pair| pair == ["-pix_fmt", "yuv420p"]));
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["-movflags", "+faststart"])
            );
        }
    }
}
