//! Video quick start: every video is either played whole or cut down to a
//! short loop (with a live animated preview) before a room is created. Cutting
//! and previewing happen with FFmpeg off the UI thread.

use crate::media_tools::{self, VideoEncoder};
use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use std::{
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

const PREVIEW_WIDTH: usize = 480;
const PREVIEW_FPS: u32 = 12;
const LENGTH_PRESETS_S: [u64; 4] = [5, 10, 15, 30];
const DEFAULT_CLIP_MS: u64 = 10_000;
const MIN_CLIP_MS: u64 = 3_000;
const MAX_CLIP_MS: u64 = 30_000;

pub enum ClipEvent {
    Idle,
    Cancel,
    /// Use the whole video as the puzzle.
    UseFull(PathBuf),
    /// The clip file is written; load it as the puzzle media.
    Ready(PathBuf),
}

pub struct ClipPlan {
    source: PathBuf,
    duration_ms: u64,
    size: (u32, u32),
    file_bytes: u64,
    has_audio: bool,
    preview: Option<Preview>,
    previewing: bool,
    preview_texture: Option<TextureHandle>,
    start_ms: u64,
    length_ms: u64,
    keep_audio: bool,
    thumb: Option<TextureHandle>,
    thumb_rx: Option<mpsc::Receiver<Result<ColorImage, String>>>,
    thumb_requested_for: Option<u64>,
    encode_rx: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    error: Option<String>,
}

impl ClipPlan {
    pub fn new(source: PathBuf, duration_ms: u64, has_audio: bool, size: (u32, u32)) -> Self {
        let length_ms = DEFAULT_CLIP_MS.min(duration_ms);
        // Start a quarter of the way in: episodes open on black cards.
        let start_ms = clamp_start(duration_ms / 4, length_ms, duration_ms);
        let file_bytes = std::fs::metadata(&source).map_or(0, |meta| meta.len());
        Self {
            file_bytes,
            size,
            preview: None,
            previewing: false,
            preview_texture: None,
            source,
            duration_ms,
            has_audio,
            start_ms,
            length_ms,
            keep_audio: has_audio,
            thumb: None,
            thumb_rx: None,
            thumb_requested_for: None,
            encode_rx: None,
            error: None,
        }
    }

    pub fn source_stem(&self) -> String {
        self.source
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Untitled puzzle")
            .replace(['.', '_'], " ")
    }

    fn request_thumbnail(&mut self, ctx: &egui::Context) {
        if self.thumb_requested_for == Some(self.start_ms) {
            return;
        }
        self.thumb_requested_for = Some(self.start_ms);
        let (tx, rx) = mpsc::channel();
        let source = self.source.clone();
        let start_ms = self.start_ms;
        let ctx = ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(grab_frame(&source, start_ms));
            ctx.request_repaint();
        });
        self.thumb_rx = Some(rx);
    }

    fn start_encode(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let output = clip_output_path(&self.source, self.start_ms);
        let source = self.source.clone();
        let (start_ms, length_ms) = (self.start_ms, self.length_ms);
        let audio = self.keep_audio && self.has_audio;
        let ctx = ctx.clone();
        thread::spawn(move || {
            // Which encoders exist depends on the FFmpeg build, so ask it.
            let encoder = VideoEncoder::choose(&media_tools::ffmpeg());
            let args = clip_ffmpeg_args(&source, &output, start_ms, length_ms, audio, encoder);
            let _ = tx.send(run_clip_encode(args, output));
            ctx.request_repaint();
        });
        self.encode_rx = Some(rx);
        self.error = None;
    }

    fn start_preview(&mut self, ctx: &egui::Context) {
        self.stop_preview();
        self.previewing = true;
        self.preview = Some(Preview::start(
            self.source.clone(),
            self.start_ms,
            self.length_ms,
            self.size,
            ctx.clone(),
        ));
    }

    fn stop_preview(&mut self) {
        self.previewing = false;
        self.preview = None; // dropping the handle stops the worker
    }

    /// Called whenever the chosen range changes.
    fn range_changed(&mut self, ctx: &egui::Context) {
        if self.previewing {
            self.start_preview(ctx);
        } else {
            self.request_thumbnail(ctx);
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) -> ClipEvent {
        let ctx = ui.ctx().clone();
        let mut event = ClipEvent::Idle;
        if let Some(rx) = &self.thumb_rx {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(image) => {
                        self.thumb =
                            Some(ctx.load_texture("clip-thumb", image, TextureOptions::LINEAR))
                    }
                    Err(error) => self.error = Some(error),
                }
                self.thumb_rx = None;
            }
        }
        if let Some(preview) = &self.preview {
            if let Some(image) = preview.frames.try_iter().last() {
                match &mut self.preview_texture {
                    Some(texture) => texture.set(image, TextureOptions::LINEAR),
                    None => {
                        self.preview_texture =
                            Some(ctx.load_texture("clip-preview", image, TextureOptions::LINEAR))
                    }
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        }
        if let Some(rx) = &self.encode_rx {
            if let Ok(result) = rx.try_recv() {
                self.encode_rx = None;
                match result {
                    Ok(path) => {
                        self.stop_preview();
                        return ClipEvent::Ready(path);
                    }
                    Err(error) => self.error = Some(error),
                }
            }
        }
        if self.thumb_requested_for.is_none() {
            self.request_thumbnail(&ctx);
        }
        let encoding = self.encode_rx.is_some();
        let too_large = self.file_bytes > crate::MAX_ROOM_MEDIA_BYTES;
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            ui.heading("How do you want to play this video?");
            ui.label(format!(
                "{} · {} · {}x{} · {}",
                self.source_stem(),
                format_clock(self.duration_ms),
                self.size.0,
                self.size.1,
                format_bytes(self.file_bytes)
            ));
        });
        ui.add_space(6.0);
        ui.vertical_centered(|ui| {
            let width = ui.available_width().min(640.0);
            let size = egui::vec2(width, width * 9.0 / 16.0);
            let shown = if self.previewing {
                self.preview_texture.as_ref()
            } else {
                self.thumb.as_ref()
            };
            match shown {
                Some(texture) => {
                    let native = texture.size_vec2();
                    let fitted = native * (size.x / native.x).min(size.y / native.y);
                    ui.image((texture.id(), fitted));
                }
                None => {
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, 4.0, egui::Color32::from_gray(24));
                    ui.put(rect, egui::Spinner::new());
                }
            }
        });
        ui.add_space(6.0);
        ui.vertical_centered(|ui| {
            ui.set_max_width(720.0);
            ui.spacing_mut().slider_width = 380.0;
            ui.horizontal(|ui| {
                ui.label("Loop length:");
                let duration_ms = self.duration_ms;
                for seconds in LENGTH_PRESETS_S
                    .into_iter()
                    .filter(|seconds| seconds * 1000 <= duration_ms)
                {
                    let length = seconds * 1000;
                    let active = self.length_ms == length;
                    if ui
                        .add_enabled(
                            !encoding,
                            egui::Button::selectable(active, format!("{seconds} s")),
                        )
                        .clicked()
                    {
                        self.length_ms = length;
                        self.start_ms =
                            clamp_start(self.start_ms, self.length_ms, self.duration_ms);
                        self.range_changed(&ctx);
                    }
                }
                if ui
                    .add_enabled(!encoding, egui::Button::new("Surprise me"))
                    .on_hover_text("Jump to a random spot")
                    .clicked()
                {
                    let span = self.duration_ms.saturating_sub(self.length_ms);
                    self.start_ms = pseudo_random(self.start_ms) % (span + 1);
                    self.range_changed(&ctx);
                }
            });
            let max_length = MAX_CLIP_MS.min(self.duration_ms);
            let mut length_s = self.length_ms as f64 / 1000.0;
            let length_response = ui.add_enabled(
                !encoding,
                egui::Slider::new(
                    &mut length_s,
                    (MIN_CLIP_MS.min(max_length) as f64 / 1000.0)..=(max_length as f64 / 1000.0),
                )
                .text("Length (s)"),
            );
            self.length_ms = (length_s * 1000.0).round() as u64;
            let max_start = self.duration_ms.saturating_sub(self.length_ms);
            let mut start_s = self.start_ms as f64 / 1000.0;
            let start_response = ui.add_enabled(
                !encoding,
                egui::Slider::new(&mut start_s, 0.0..=(max_start as f64 / 1000.0))
                    .custom_formatter(|value, _| format_clock((value * 1000.0) as u64))
                    .text("Start"),
            );
            self.start_ms = clamp_start(
                (start_s * 1000.0).round() as u64,
                self.length_ms,
                self.duration_ms,
            );
            let dragging = start_response.dragged() || length_response.dragged();
            if ((start_response.changed() || length_response.changed()) && !dragging)
                || start_response.drag_stopped()
                || length_response.drag_stopped()
            {
                self.range_changed(&ctx);
            }
            ui.label(format!(
                "Loop: {} to {}",
                format_clock(self.start_ms),
                format_clock(self.start_ms + self.length_ms)
            ));
            ui.horizontal(|ui| {
                let label = if self.previewing {
                    "Stop preview"
                } else {
                    "Preview loop"
                };
                if ui
                    .add_enabled(!encoding, egui::Button::new(label))
                    .clicked()
                {
                    if self.previewing {
                        self.stop_preview();
                    } else {
                        self.start_preview(&ctx);
                    }
                }
                ui.weak("(preview is silent)");
                if self.has_audio {
                    ui.checkbox(&mut self.keep_audio, "Keep audio");
                }
            });
            if let Some(error) = &self.error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if encoding {
                    ui.spinner();
                    ui.label("Cutting clip…");
                } else {
                    if ui.button("Make puzzle clip").clicked() {
                        self.start_encode(&ctx);
                    }
                    let full_label =
                        format!("Use the full video ({})", format_bytes(self.file_bytes));
                    if ui
                        .add_enabled(!too_large, egui::Button::new(full_label))
                        .on_hover_text("Loop the whole video; the file is uploaded as-is")
                        .clicked()
                    {
                        self.stop_preview();
                        event = ClipEvent::UseFull(self.source.clone());
                    }
                    if ui.button("Cancel").clicked() {
                        self.stop_preview();
                        event = ClipEvent::Cancel;
                    }
                }
            });
            if too_large {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!(
                        "This file is over the {} room limit; make a clip instead.",
                        crate::package::format_bytes(crate::MAX_ROOM_MEDIA_BYTES)
                    ),
                );
            } else if self.file_bytes > 500 * 1024 * 1024 {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Large file: the full video uploads whole and may take a while.",
                );
            }
        });
        event
    }
}

/// Handle for the silent looping preview worker; dropping it stops FFmpeg.
struct Preview {
    frames: mpsc::Receiver<ColorImage>,
    stop: Arc<AtomicBool>,
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Preview {
    fn start(
        source: PathBuf,
        start_ms: u64,
        length_ms: u64,
        size: (u32, u32),
        ctx: egui::Context,
    ) -> Self {
        let (tx, frames) = mpsc::sync_channel(2);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        thread::spawn(move || run_preview(source, start_ms, length_ms, size, tx, worker_stop, ctx));
        Self { frames, stop }
    }
}

fn preview_dimensions(size: (u32, u32)) -> (usize, usize) {
    let height =
        (PREVIEW_WIDTH as f64 * size.1.max(1) as f64 / size.0.max(1) as f64).round() as usize;
    (PREVIEW_WIDTH, (height.max(2) / 2) * 2)
}

fn run_preview(
    source: PathBuf,
    start_ms: u64,
    length_ms: u64,
    size: (u32, u32),
    tx: mpsc::SyncSender<ColorImage>,
    stop: Arc<AtomicBool>,
    ctx: egui::Context,
) {
    let (width, height) = preview_dimensions(size);
    let mut buffer = vec![0u8; width * height * 4];
    while !stop.load(Ordering::Relaxed) {
        let Ok(mut child) = Command::new(media_tools::ffmpeg())
            .args(["-v", "error", "-an", "-ss"])
            .arg(format!("{:.3}", start_ms as f64 / 1000.0))
            .args(["-readrate", "1", "-t"])
            .arg(format!("{:.3}", length_ms as f64 / 1000.0))
            .arg("-i")
            .arg(&source)
            .args(["-vf", &format!("scale={width}:{height},fps={PREVIEW_FPS}")])
            .args(["-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return;
        };
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill();
            return;
        };
        let mut frames_read = 0;
        while !stop.load(Ordering::Relaxed) && stdout.read_exact(&mut buffer).is_ok() {
            frames_read += 1;
            let image = ColorImage::from_rgba_unmultiplied([width, height], &buffer);
            if tx.send(image).is_err() {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            ctx.request_repaint();
        }
        let _ = child.kill();
        let _ = child.wait();
        if frames_read == 0 {
            return; // undecodable range; avoid a spin loop
        }
    }
}

fn pseudo_random(seed: u64) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    (nanos ^ seed.wrapping_mul(0x9e37_79b9_7f4a_7c15)).wrapping_mul(0xff51_afd7_ed55_8ccd) >> 16
}

pub fn format_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    let value = bytes as f64;
    if value >= 1024.0 * MIB {
        format!("{:.1} GB", value / (1024.0 * MIB))
    } else if value >= MIB {
        format!("{:.0} MB", value / MIB)
    } else {
        format!("{:.0} KB", (value / 1024.0).max(1.0))
    }
}

fn clamp_start(start_ms: u64, length_ms: u64, duration_ms: u64) -> u64 {
    start_ms.min(duration_ms.saturating_sub(length_ms))
}

pub fn format_clock(ms: u64) -> String {
    let total = ms / 1000;
    if total >= 3600 {
        format!("{}:{:02}:{:02}", total / 3600, total / 60 % 60, total % 60)
    } else {
        format!("{}:{:02}", total / 60, total % 60)
    }
}

fn clip_output_path(source: &Path, start_ms: u64) -> PathBuf {
    let stem: String = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("clip")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(40)
        .collect();
    crate::client_data_dir()
        .join("clips")
        .join(format!("{stem}-{}s.mp4", start_ms / 1000))
}

fn clip_ffmpeg_args(
    source: &Path,
    output: &Path,
    start_ms: u64,
    length_ms: u64,
    audio: bool,
    encoder: VideoEncoder,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "-v".into(),
        "error".into(),
        "-y".into(),
        "-ss".into(),
        format!("{:.3}", start_ms as f64 / 1000.0).into(),
        "-i".into(),
        source.into(),
        "-t".into(),
        format!("{:.3}", length_ms as f64 / 1000.0).into(),
        "-map".into(),
        "0:v:0".into(),
    ];
    if audio {
        args.extend(["-map", "0:a:0?", "-c:a", "aac", "-b:a", "128k"].map(OsString::from));
    } else {
        args.push("-an".into());
    }
    args.extend(["-vf", "scale=trunc(min(1280\\,iw)/2)*2:-2"].map(OsString::from));
    args.extend(encoder.args().into_iter().map(OsString::from));
    args.push(output.into());
    args
}

fn run_clip_encode(args: Vec<OsString>, output: PathBuf) -> Result<PathBuf, String> {
    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("Could not create clip folder: {error}"))?;
    }
    let result = Command::new(media_tools::ffmpeg())
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|_| {
            "FFmpeg was not found. Install it, or put it next to PieceOff, to cut clips.".to_owned()
        })?;
    if !result.status.success() {
        let detail = String::from_utf8_lossy(&result.stderr);
        let detail = detail.lines().last().unwrap_or("unknown error");
        return Err(format!("FFmpeg could not cut the clip: {detail}"));
    }
    Ok(output)
}

fn grab_frame(source: &Path, at_ms: u64) -> Result<ColorImage, String> {
    let output = Command::new(media_tools::ffmpeg())
        .args(["-v", "error", "-ss"])
        .arg(format!("{:.3}", at_ms as f64 / 1000.0))
        .arg("-i")
        .arg(source)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale=960:-2",
            "-f",
            "image2",
            "-c:v",
            "png",
            "pipe:1",
        ])
        .stderr(Stdio::null())
        .output()
        .map_err(|_| {
            "FFmpeg was not found. Install it, or put it next to PieceOff, to preview clips."
                .to_owned()
        })?;
    let image = image::load_from_memory(&output.stdout)
        .map_err(|_| "Could not read a preview frame at that position.".to_owned())?
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use media_tools::VideoEncoder;

    #[test]
    fn clock_formats_minutes_and_hours() {
        assert_eq!(format_clock(0), "0:00");
        assert_eq!(format_clock(734_000), "12:14");
        assert_eq!(format_clock(3_725_000), "1:02:05");
    }

    #[test]
    fn preview_keeps_aspect_with_even_height() {
        assert_eq!(preview_dimensions((1920, 1080)), (480, 270));
        assert_eq!(preview_dimensions((640, 480)), (480, 360));
        assert_eq!(preview_dimensions((1000, 999)).1 % 2, 0);
    }

    #[test]
    fn byte_sizes_read_naturally() {
        assert_eq!(format_bytes(300 * 1024 * 1024), "300 MB");
        assert_eq!(format_bytes(1536 * 1024 * 1024), "1.5 GB");
        assert_eq!(format_bytes(180 * 1024), "180 KB");
        assert_eq!(format_bytes(0), "1 KB");
    }

    #[test]
    fn start_never_runs_past_the_end() {
        assert_eq!(clamp_start(100_000, 10_000, 60_000), 50_000);
        assert_eq!(clamp_start(5_000, 10_000, 8_000), 0);
    }

    #[test]
    fn default_plan_skips_the_opening_and_fits_the_video() {
        let plan = ClipPlan::new("episode.mkv".into(), 1_300_000, true, (1920, 1080));
        assert_eq!(plan.length_ms, 10_000);
        assert_eq!(plan.start_ms, 325_000);
        let short = ClipPlan::new("x.mkv".into(), 4_000, false, (640, 360));
        assert_eq!((short.start_ms, short.length_ms), (0, 4_000));
    }

    #[test]
    fn ffmpeg_args_cut_one_video_stream_and_optional_audio() {
        let with_audio = clip_ffmpeg_args(
            Path::new("a.mkv"),
            Path::new("o.mp4"),
            12_500,
            10_000,
            true,
            VideoEncoder::X264,
        );
        let text: Vec<_> = with_audio
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(text.windows(2).any(|w| w == ["-ss", "12.500"]));
        assert!(text.windows(2).any(|w| w == ["-t", "10.000"]));
        assert!(text.contains(&"0:a:0?".to_owned()));
        let silent = clip_ffmpeg_args(
            Path::new("a.mkv"),
            Path::new("o.mp4"),
            0,
            5_000,
            false,
            VideoEncoder::Mpeg4,
        );
        assert!(silent.iter().any(|a| a == "-an"));
    }

    #[test]
    fn cuts_a_real_clip_from_a_generated_video() {
        let dir = std::env::temp_dir().join(format!("pieceoff-clip-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("src.mp4");
        let made = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=10",
                "-t",
                "6",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&source)
            .status();
        if !made.is_ok_and(|s| s.success()) {
            return; // FFmpeg unavailable
        }
        let output = dir.join("out.mp4");
        let args = clip_ffmpeg_args(
            &source,
            &output,
            1_000,
            3_000,
            false,
            VideoEncoder::choose(&media_tools::ffmpeg()),
        );
        run_clip_encode(args, output.clone()).unwrap();
        assert!(std::fs::metadata(&output).unwrap().len() > 0);
        let frame = grab_frame(&source, 2_000).unwrap();
        assert_eq!(frame.size, [960, 540]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
