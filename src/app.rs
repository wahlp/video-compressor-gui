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

const PROGRAM_CONFIG_NAME: &str = "video_compressor_gui";

// ffmpeg encoder parameter
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum Encoder {
    CpuX264,
    GpuNvenc,
}

impl Default for Encoder {
    fn default() -> Self {
        Encoder::CpuX264
    }
}

// compression options
#[derive(Serialize, Deserialize)]
pub struct AppConfig {
    pub target_size_mb: u32,
    pub frame_rate: Option<u32>,

    #[serde(default)]
    pub encoder: Encoder,
}

impl ::std::default::Default for AppConfig {
    fn default() -> Self {
        Self {
            target_size_mb: 10,
            frame_rate: None,
            encoder: Encoder::CpuX264
        }
    }
}

#[derive(PartialEq, Clone)]
pub enum FileStatus {
    Waiting,
    Processing,
    Done,
}

#[derive(Clone)]
pub struct QueueItem {
    pub path: PathBuf,
    pub status: FileStatus,
    pub size_bytes: u64,
}

pub enum Tab {
    Main,
    Options,
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
            config: confy::load(PROGRAM_CONFIG_NAME, None)?,
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

        let target_size_mb = self.config.target_size_mb * 1000;

        let log_arc = Arc::clone(&self.ffmpeg_log);
        let busy_flag = Arc::clone(&self.ffmpeg_busy);
        let should_start_next_clone = Arc::clone(&self.should_start_next);
        let video_queue_clone = Arc::clone(&self.video_queue);
        let frame_rate_option = self.config.frame_rate;
        let encoder = self.config.encoder.clone();

        thread::spawn(move || {

            let Some((video_bitrate, audio_bitrate)) = calculate_bitrate(queue_item.to_str().unwrap(), target_size_mb) else {
                log_tx.send("Failed to calculate bitrate.".to_string()).ok();
                log_tx.send("[done]".to_string()).ok();
                return;
            };

            let b_v = format!("{}", video_bitrate);
            let b_a = format!("{}", audio_bitrate);

            let output_path = queue_item.with_extension("compressed.mp4");
            let mut args = vec![
                "-i", queue_item.to_str().unwrap(),
                "-c:v",
                match encoder {
                    Encoder::CpuX264 => "libx264",
                    Encoder::GpuNvenc => "h264_nvenc",
                },
                "-b:v", &b_v,
                "-c:a", "aac",
                "-b:a", &b_a,
                "-y", output_path.to_str().unwrap(),
            ];

            let fps_opt = frame_rate_option.map(|fps| format!("fps={}", fps));
            if let Some(fps_str) = fps_opt.as_deref() {
                args.splice(2..2, ["-filter:v", fps_str]);
            }

            // Print the full command string to the log
            let cmd_string = format!("ffmpeg {}", args.iter()
                .map(|s| shell_quote(s))
                .collect::<Vec<_>>()
                .join(" ")
            );
            log_tx.send(format!("Running command: {}", cmd_string)).ok();

            let mut cmd = Command::new("ffmpeg")
                .args(args)
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

// read input video file's parameters to calculate output file's parameters later
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
    
    let bitrate = lines.next()?.trim().parse::<u32>().ok()?;
    let duration = lines.next()?.trim().parse::<f64>().ok()?;
    
    Some((duration, bitrate))
}

fn calculate_bitrate(video_path: &str, size_upper_bound_mb: u32) -> Option<(u32, u32)> {
    let (duration, mut audio_bitrate) = get_duration_and_audio_bitrate(video_path)?;

    // calculate the allowed bits per second to reach target output file size
    let gib_to_gb_conversion = 1.073741824;
    let target_total_bitrate = (size_upper_bound_mb * 1024 * 8) as f64 / (gib_to_gb_conversion * duration);

    // allocate some bitrate for audio
    let min_audio_bitrate = 64000;
    let max_audio_bitrate = 256000;
    if 10.0 * audio_bitrate as f64 > target_total_bitrate {
        audio_bitrate = (target_total_bitrate / 10.0) as u32;
        audio_bitrate = audio_bitrate.clamp(min_audio_bitrate, max_audio_bitrate)
    }
    
    // spend the remaining bitrate on video
    let video_bitrate = (target_total_bitrate as u32).saturating_sub(audio_bitrate);

    Some((video_bitrate, audio_bitrate))
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        ctx.set_zoom_factor(1.2);
        
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
                if ui.selectable_label(matches!(self.current_tab, Tab::Options), "Options").clicked() {
                    self.current_tab = Tab::Options;
                }
                if ui.selectable_label(matches!(self.current_tab, Tab::Output), "Debug Output").clicked() {
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
                            if let Ok(metadata) = std::fs::metadata(&path) {
                                let size_bytes = metadata.len();
                                self.video_queue.lock().unwrap().push(QueueItem {
                                    path,
                                    size_bytes,
                                    status: FileStatus::Waiting,
                                });
                            }
                        }
                    }

                    let queue = self.video_queue.lock().unwrap().clone();
                    if queue.is_empty() {
                        // Add space to center vertically
                        let available_height = ui.available_height();
                        let prompt_height = 120.0;
                        ui.add_space((available_height - prompt_height) / 2.0);

                        ui.vertical_centered(|ui| {
                            ui.label(egui::RichText::new("📁").size(40.0));
                            ui.label(egui::RichText::new("Drop video files here to begin").heading().weak());
                        });
                    } else {
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
                        egui::Grid::new("queue_grid")
                            .striped(true)
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new("Status").strong());
                                ui.label(egui::RichText::new("Filename").strong());
                                ui.label(egui::RichText::new("Size").strong());
                                ui.end_row();

                                for item in queue.iter() {
                                    let emoji = match item.status {
                                        FileStatus::Waiting => "🕓",
                                        FileStatus::Processing => "🔄",
                                        FileStatus::Done => "✅",
                                    };
                                    ui.label(emoji);
                                    ui.label(item.path.file_name().unwrap_or_default().to_string_lossy());
                                    ui.label(format_size(item.size_bytes));
                                    ui.end_row();
                                }
                            });
                    }
                }

                Tab::Options => {
                    ui.horizontal(|ui| {
                        ui.label("Target size (MB):");
                        if ui.add(egui::DragValue::new(&mut self.config.target_size_mb)).changed() {
                            self.config_dirty = true;
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.label("Frame rate (optional):");
                        let mut fr_string = self.config.frame_rate.map(|v| v.to_string()).unwrap_or_default();
                        if ui.add_sized(
                                egui::vec2(40.0, 20.0),
                                egui::TextEdit::singleline(&mut fr_string)
                            ).changed() 
                        {
                            self.config.frame_rate = fr_string.trim().parse().ok();
                            self.config_dirty = true;
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.label("Encoder:");
                        ui.selectable_value(&mut self.config.encoder, Encoder::CpuX264, "CPU");
                        ui.selectable_value(&mut self.config.encoder, Encoder::GpuNvenc, "GPU, faster but larger files than CPU");
                    });

                    if self.config_dirty {
                        confy::store(PROGRAM_CONFIG_NAME, None, &self.config).ok();
                        self.config_dirty = false;
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

fn shell_quote(arg: &str) -> String {
    if arg.contains(' ') || arg.contains('"') || arg.contains('\'') {
        // Escape existing quotes by backslash for safety (basic)
        let escaped = arg.replace('"', "\\\"");
        format!("\"{}\"", escaped)
    } else {
        arg.to_string()
    }
}

fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let b = bytes as f64;
    if b < KB {
        format!("{:.0} B", b)
    } else if b < MB {
        format!("{:.1} KB", b / KB)
    } else if b < GB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.2} GB", b / GB)
    }
}
