use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex, atomic::{AtomicBool, Ordering}},
    thread,
};
use std::io::{BufRead, BufReader};
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

#[derive(PartialEq)]
pub enum FileStatus {
    Waiting,
    Processing,
    Done,
}

pub struct QueueItem {
    pub path: PathBuf,
    pub status: FileStatus,
}

pub enum Tab {
    Main,
    Output,
}

pub struct MyApp {
    config: AppConfig,
    config_dirty: bool,
    video_queue: Arc<Mutex<Vec<QueueItem>>>,
    ffmpeg_log: Arc<Mutex<Vec<String>>>,
    ffmpeg_busy: Arc<AtomicBool>,
    tx: Option<Sender<String>>,
    should_start_next: Arc<Mutex<bool>>,
    current_tab: Tab,
}

impl MyApp {
    pub fn load() -> Result<Self, confy::ConfyError> {
        Ok(Self {
            config: confy::load("video_compressor_gui", None)?,
            config_dirty: false,
            video_queue: Arc::new(Mutex::new(Vec::new())),
            ffmpeg_log: Arc::new(Mutex::new(Vec::new())),
            ffmpeg_busy: Arc::new(AtomicBool::new(false)),
            tx: None,
            should_start_next: Arc::new(Mutex::new(false)),
            current_tab: Tab::Main,
        })
    }

    fn start_ffmpeg_thread(&mut self) {
        if self.ffmpeg_busy.load(Ordering::SeqCst) {
            return;
        }

        let queue_item_path = {
            let mut queue = match self.video_queue.lock() {
                Ok(q) => q,
                Err(_) => return,
            };
            if let Some(item) = queue.iter_mut().find(|i| matches!(i.status, FileStatus::Waiting)) {
                item.status = FileStatus::Processing;
                Some(item.path.clone())
            } else {
                None
            }
        };

        let Some(queue_item) = queue_item_path else {
            return;
        };

        let queue_item_clone = queue_item.clone();

        let (log_tx, log_rx): (Sender<String>, Receiver<String>) = mpsc::channel();

        self.ffmpeg_busy.store(true, Ordering::SeqCst);
        self.tx = Some(log_tx.clone());

        let target_size = self.config.target_size_mb * 1000;

        let log_arc = Arc::clone(&self.ffmpeg_log);
        let busy_flag = Arc::clone(&self.ffmpeg_busy);
        let should_start_next_clone = Arc::clone(&self.should_start_next);
        let video_queue_clone = Arc::clone(&self.video_queue);

        // Spawn thread to run FFmpeg
        thread::spawn(move || {
            let output_path = queue_item.with_extension("compressed.mp4");

            let Some((video_bitrate, audio_bitrate)) = calculate_bitrate(queue_item.to_str().unwrap(), target_size) else {
                log_tx.send("Failed to calculate bitrate.".to_string()).ok();
                log_tx.send("[done]".to_string()).ok();
                return;
            };

            let mut cmd = Command::new("ffmpeg")
                .args([
                    "-i", queue_item.to_str().unwrap(),
                    "-r", "60",
                    "-c:v", "libx264",
                    "-b:v", &format!("{}", video_bitrate),
                    "-c:a", "aac",
                    "-b:a", &format!("{}", audio_bitrate),
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

        // Handle log output + status change
        thread::spawn(move || {
            while let Ok(line) = log_rx.recv() {
                if line == "[done]" {
                    busy_flag.store(false, Ordering::SeqCst);
                    if let Ok(mut queue) = video_queue_clone.lock() {
                        if let Some(item) = queue.iter_mut().find(|i| i.path == queue_item_clone) {
                            item.status = FileStatus::Done;
                        }
                    }
                    if let Ok(mut flag) = should_start_next_clone.lock() {
                        *flag = true;
                    }
                } else {
                    if let Ok(mut log) = log_arc.lock() {
                        log.push(line);
                    }
                }
            }
        });
    }
}

fn get_duration_and_audio_bitrate(path: &str) -> Option<(f64, u32)> {
    let output = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "a:0",
            "-show_entries", "format=duration:stream=bit_rate",
            "-of", "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    
    let bitrate = lines.next()?.trim().parse::<u32>().ok()?; // in bits/sec
    let duration = lines.next()?.trim().parse::<f64>().ok()?;
    
    Some((duration, bitrate))
}

fn calculate_bitrate(video_path: &str, size_upper_bound: u32) -> Option<(u32, u32)> {
    let (duration, mut audio_bitrate) = get_duration_and_audio_bitrate(video_path)?;
    let gib_to_gb_conversion = 1.073741824;
    let target_total_bitrate = (size_upper_bound * 1024 * 8) as f64 / (gib_to_gb_conversion * duration);

    let min_audio_bitrate = 64000;
    let max_audio_bitrate = 256000;
    if 10.0 * audio_bitrate as f64 > target_total_bitrate {
        audio_bitrate = (target_total_bitrate / 10.0) as u32;
        if audio_bitrate < min_audio_bitrate as u32 {
            audio_bitrate = min_audio_bitrate as u32;
        } else if audio_bitrate > max_audio_bitrate as u32 {
            audio_bitrate = max_audio_bitrate as u32;
        }
    }
    
    let video_bitrate = (target_total_bitrate as u32).saturating_sub(audio_bitrate);

    Some((video_bitrate, audio_bitrate))
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        // Automatically start next compression job if flagged
        if !self.ffmpeg_busy.load(Ordering::SeqCst) && !self.video_queue.lock().unwrap().is_empty() {
            let should_start = {
                if let Ok(mut flag) = self.should_start_next.lock() {
                    if *flag {
                        *flag = false;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            };

            if should_start {
                self.start_ffmpeg_thread();
            }
        }

        // Draw top tab bar
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.selectable_label(matches!(self.current_tab, Tab::Main), "Main").clicked() {
                    self.current_tab = Tab::Main;
                }
                if ui.selectable_label(matches!(self.current_tab, Tab::Output), "FFmpeg Output").clicked() {
                    self.current_tab = Tab::Output;
                }
            });
        });

        // Main panel based on current tab
        egui::CentralPanel::default().show(ctx, |ui| {
            match self.current_tab {
                Tab::Main => {
                    // Drag & drop handler
                    for file in ctx.input(|i| i.raw.dropped_files.clone()) {
                        if let Some(path) = file.path {
                            self.video_queue.lock().unwrap().push(QueueItem {
                                path,
                                status: FileStatus::Waiting,
                            });
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

                    if ui
                        .add_sized(
                            egui::vec2(200.0, 40.0),
                            egui::Button::new(egui::RichText::new("Start Compression").strong()).wrap(),
                        )
                        .clicked()
                    {
                        self.start_ffmpeg_thread();
                    }

                    ui.separator();
                    ui.label("Queue:");

                    let queue = self.video_queue.lock().unwrap();
                    if queue.is_empty() {
                        let available_height = ui.available_height();
                        let prompt_height = 100.0; // approximate height of the prompt

                        // Add space to center vertically
                        ui.add_space((available_height - prompt_height) / 2.0);
                        ui.vertical_centered(|ui| {
                            ui.label(egui::RichText::new("📁").size(40.0));
                            ui.label(egui::RichText::new("Drop video files here to begin").heading().weak());
                        });
                    } else {
                        egui::Grid::new("queue_grid")
                            .striped(true)
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new("📋 Status").strong());
                                ui.label(egui::RichText::new("📁 Filename").strong());
                                ui.end_row();

                                for item in queue.iter() {
                                    let emoji = match item.status {
                                        FileStatus::Waiting => "🕓",
                                        FileStatus::Processing => "🔄",
                                        FileStatus::Done => "✅",
                                    };
                                    ui.label(emoji);
                                    ui.label(item.path.file_name().unwrap_or_default().to_string_lossy());
                                    ui.end_row();
                                }
                            });
                    }
                }

                Tab::Output => {
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            if let Ok(log) = self.ffmpeg_log.lock() {
                                for line in log.iter() {
                                    ui.label(line);
                                }
                            }
                        });
                }
            }
        });

        ctx.request_repaint();
    }
}
