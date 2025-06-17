use std::{
    path::PathBuf,
    sync::{mpsc, Arc, Mutex, atomic::{AtomicBool, Ordering}},
    thread,
};
use std::sync::mpsc::{Sender, Receiver};
use serde::{Serialize, Deserialize};
use confy;
use eframe::egui;

#[derive(Serialize, Deserialize)]
pub struct AppConfig {
    pub target_size_mb: u32,
}

impl ::std::default::Default for AppConfig {
    fn default() -> Self {
        Self {
            target_size_mb: 10,
        }
    }
}

pub struct MyApp {
    config: AppConfig,
    config_dirty: bool,
    video_queue: Vec<PathBuf>,
    ffmpeg_log: Arc<Mutex<Vec<String>>>,
    ffmpeg_busy: Arc<AtomicBool>,
    tx: Option<Sender<String>>,
}

impl MyApp {
    pub fn load() -> Result<Self, confy::ConfyError> {
        Ok(Self {
            config: confy::load("video_compressor_gui", None)?,
            config_dirty: false,
            video_queue: vec![],
            ffmpeg_log: Arc::new(Mutex::new(Vec::new())),
            ffmpeg_busy: Arc::new(AtomicBool::new(false)),
            tx: None,
        })
    }

    fn start_ffmpeg_thread(&mut self) {
        if self.ffmpeg_busy.load(Ordering::SeqCst) || self.video_queue.is_empty() {
            return;
        }

        let path = self.video_queue.remove(0);
        let (log_tx, log_rx): (Sender<String>, Receiver<String>) = mpsc::channel();

        self.ffmpeg_busy.store(true, Ordering::SeqCst);
        self.tx = Some(log_tx.clone());

        let target_size = self.config.target_size_mb;

        // Spawn thread to run FFmpeg
        thread::spawn(move || {
            use std::process::{Command, Stdio};
            use std::io::{BufRead, BufReader};

            let output_path = path.with_extension("compressed.mp4");

            // calculate desired bitrate
            let bitrate_kbps = (target_size * 8192) / 60;

            let mut cmd = Command::new("ffmpeg")
                .args([
                    "-i", path.to_str().unwrap(),
                    "-r", "60",
                    "-c:v", "libx264",
                    "-b:v", &format!("{}k", bitrate_kbps),
                    "-c:a", "aac",
                    "-b:a", "128k",                      // or adjust to match source
                    "-y", output_path.to_str().unwrap(),
                ])
                .stderr(Stdio::piped())
                .spawn()
                .expect("failed to run ffmpeg");

            let stderr = cmd.stderr.take().unwrap();
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if let Ok(line) = line {
                    log_tx.send(line).ok();
                }
            }

            cmd.wait().ok();
            log_tx.send("[done]".to_string()).ok();
        });

        // Clone log and busy flag for the log reading thread
        let log_arc = Arc::clone(&self.ffmpeg_log);
        let busy_flag = Arc::clone(&self.ffmpeg_busy);

        thread::spawn(move || {
            while let Ok(line) = log_rx.recv() {
                if line == "[done]" {
                    busy_flag.store(false, Ordering::SeqCst);
                } else {
                    if let Ok(mut log) = log_arc.lock() {
                        log.push(line);
                    }
                }
            }
        });
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("FFmpeg Video Compressor");

            // Drag & drop handler
            for file in ctx.input(|i| i.raw.dropped_files.clone()) {
                if let Some(path) = file.path {
                    self.video_queue.push(path);
                }
            }

            ui.horizontal(|ui| {
                ui.label("Target size (MB):");
                if ui.add(egui::DragValue::new(&mut self.config.target_size_mb)).changed() {
                    self.config_dirty = true;
                }

                if self.config_dirty {
                    confy::store("video_compressor_gui", None, &self.config).ok();
                    self.config_dirty = false;
                }
            });

            if ui.button("Start Compression").clicked() {
                self.start_ffmpeg_thread();
            }

            ui.separator();
            ui.label("Queue:");
            for file in &self.video_queue {
                ui.label(file.to_string_lossy());
            }

            ui.separator();
            ui.label("FFmpeg Output:");
            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Ok(log) = self.ffmpeg_log.lock() {
                    for line in log.iter() {
                        ui.label(line);
                    }
                }
            });
        });

        ctx.request_repaint(); // keep UI responsive
    }
}
