//! The offer to install the video codecs: when it appears, the download,
//! and the window that asks first. The download itself is in `codec_pack`.

use super::*;
use crate::codec_pack::{self, Pack};

/// Codecs the one-click install exists for; a build without them is the
/// bundled one.
const PATENTED: [&str; 3] = ["h264", "hevc", "aac"];

#[derive(Default)]
pub(crate) struct CodecState {
    prompt: Option<CodecPrompt>,
    install: Option<CodecInstall>,
    /// Why the last install failed.
    error: Option<String>,
    /// A video the player chose to open without installing anything.
    skip_check: Option<PathBuf>,
}

struct CodecPrompt {
    reason: String,
    /// The video that triggered the offer, to reopen once installed.
    resume: Option<PathBuf>,
}

struct CodecInstall {
    rx: mpsc::Receiver<CodecEvent>,
    cancel: Arc<AtomicBool>,
    done: u64,
    total: u64,
    resume: Option<PathBuf>,
}

enum CodecEvent {
    Progress(u64, u64),
    Finished(Result<(), String>),
}

impl PieceOffApp {
    /// True (and opens the offer) when the video has codecs (`missing`) the
    /// current FFmpeg cannot decode and the player has not waved the offer away.
    pub(crate) fn needs_codec_install(&mut self, path: &Path, missing: &[String]) -> bool {
        if missing.is_empty() || self.codec.skip_check.as_deref() == Some(path) {
            return false;
        }
        let names: Vec<String> = missing
            .iter()
            .map(|name| media_tools::codec_label(name))
            .collect();
        self.status = "This video needs video codecs.".into();
        self.open_codec_prompt(
            format!(
                "This video uses {}, which this copy of FFmpeg cannot play.",
                names.join(" and ")
            ),
            Some(path.to_path_buf()),
        );
        true
    }

    fn ffmpeg_decoders(&self) -> Option<&HashSet<String>> {
        self.tools
            .iter()
            .find(|report| report.tool == media_tools::Tool::Ffmpeg)
            .and_then(|report| report.decoders.as_ref())
    }

    pub(crate) fn open_codec_prompt(&mut self, reason: String, resume: Option<PathBuf>) {
        self.codec.prompt = Some(CodecPrompt { reason, resume });
    }

    fn start_codec_install(&mut self) {
        let Some(pack) = Pack::current() else { return };
        let resume = self.codec.prompt.take().and_then(|prompt| prompt.resume);
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (worker_cancel, repaint) = (cancel.clone(), self.repaint.clone());
        let total = pack.total_bytes();
        thread::spawn(move || {
            let wake = || {
                if let Some(repaint) = &repaint {
                    repaint.request_repaint();
                }
            };
            let mut last_sent = 0;
            let result = codec_pack::install(
                &pack,
                &codec_pack::install_dir(),
                &worker_cancel,
                |done, total| {
                    // Reads come in 64 KB pieces; the bar only needs a few updates a second.
                    if done == total || done >= last_sent + 512 * 1024 {
                        last_sent = done;
                        let _ = tx.send(CodecEvent::Progress(done, total));
                        wake();
                    }
                },
            );
            let _ = tx.send(CodecEvent::Finished(result));
            wake();
        });
        self.codec.install = Some(CodecInstall {
            rx,
            cancel,
            done: 0,
            total,
            resume,
        });
        self.codec.error = None;
    }

    pub(crate) fn poll_codec_install(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.codec.install else {
            return;
        };
        let mut finished = None;
        while let Ok(event) = job.rx.try_recv() {
            match event {
                CodecEvent::Progress(done, total) => (job.done, job.total) = (done, total),
                CodecEvent::Finished(result) => finished = Some(result),
            }
        }
        let Some(result) = finished else { return };
        let resume = job.resume.take();
        self.codec.install = None;
        match result {
            Ok(()) => {
                self.refresh_tools();
                let active = media_tools::resolve(media_tools::Tool::Ffmpeg);
                self.decoder_ffmpeg_program = active.program;
                self.status = match active.source {
                    // A folder the player chose outranks the install.
                    media_tools::ToolSource::Chosen | media_tools::ToolSource::Environment => {
                        "Video codecs installed, but the FFmpeg you chose is still in use. \
                         Pick \"Use default\" under Media tools to switch."
                            .into()
                    }
                    _ => "Video codecs installed.".into(),
                };
                if let Some(path) = resume {
                    // Whatever is active now is what the player wants; do not ask again.
                    self.codec.skip_check = Some(path.clone());
                    self.begin_probe(path, false, ctx);
                }
            }
            Err(error) => {
                self.status = "Video codecs were not installed.".into();
                self.codec.error = Some(error);
                self.open_codec_prompt("The install did not finish.".into(), resume);
            }
        }
    }

    /// The consent window and, once agreed, the progress window.
    pub(crate) fn codec_window(&mut self, ctx: &egui::Context) {
        if self.codec.prompt.is_none() && self.codec.install.is_none() {
            return;
        }
        let pack = Pack::current();
        let (mut install, mut dismiss, mut anyway, mut cancel) = (false, false, false, false);
        egui::Window::new("Video codecs")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_max_width(460.0);
                if let Some(job) = &self.codec.install {
                    ui.label("Downloading video codecs…");
                    let fraction = if job.total == 0 {
                        0.0
                    } else {
                        job.done as f32 / job.total as f32
                    };
                    ui.add(egui::ProgressBar::new(fraction).show_percentage());
                    ui.weak(format!(
                        "{} of {} MB",
                        job.done / 1_000_000,
                        job.total / 1_000_000
                    ));
                    cancel = ui.button("Cancel").clicked();
                    return;
                }
                let Some(prompt) = &self.codec.prompt else {
                    return;
                };
                ui.strong(&prompt.reason);
                ui.add_space(6.0);
                match &pack {
                    Some(pack) => {
                        ui.label(pack.consent_text());
                    }
                    None => {
                        ui.label(
                            "There is no one-click install for this computer. Install FFmpeg \
                             yourself (the ffmpeg and ffprobe programs), then choose its folder \
                             under You → Media tools.",
                        );
                    }
                }
                if let Some(error) = &self.codec.error {
                    ui.add_space(6.0);
                    ui.colored_label(WARNING, error);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if pack.is_some() && ui.button("Install video codecs").clicked() {
                        install = true;
                    }
                    if prompt.resume.is_some() && ui.button("Open anyway").clicked() {
                        anyway = true;
                    }
                    if ui.button("Not now").clicked() {
                        dismiss = true;
                    }
                });
            });
        if cancel {
            if let Some(job) = &self.codec.install {
                job.cancel.store(true, Ordering::Relaxed);
            }
        }
        if install {
            self.start_codec_install();
        } else if anyway {
            if let Some(path) = self.codec.prompt.take().and_then(|prompt| prompt.resume) {
                self.codec.skip_check = Some(path.clone());
                self.begin_probe(path, false, ctx);
            }
        } else if dismiss {
            self.codec.prompt = None;
            self.codec.error = None;
        }
    }

    /// The codec line in the Media tools panel.
    pub(crate) fn codec_status_ui(&mut self, ui: &mut egui::Ui) {
        let Some(decoders) = self.ffmpeg_decoders() else {
            return;
        };
        let lacking = !media_tools::undecodable(decoders, &PATENTED.map(str::to_owned)).is_empty();
        let installed_dir = codec_pack::install_dir();
        if lacking {
            ui.colored_label(
                WARNING,
                "Video codecs: H.264, HEVC and AAC are not installed.",
            );
            if ui.button("Install video codecs…").clicked() {
                self.open_codec_prompt(
                    "Most MP4, MOV and MKV videos need these codecs.".into(),
                    None,
                );
            }
        } else {
            ui.label("Video codecs: H.264, HEVC and AAC are available.");
        }
        if installed_dir.is_dir() && ui.button("Remove downloaded codecs").clicked() {
            let _ = std::fs::remove_dir_all(&installed_dir);
            self.decoder_ffmpeg_program = media_tools::ffmpeg();
            self.refresh_tools();
        }
    }
}
