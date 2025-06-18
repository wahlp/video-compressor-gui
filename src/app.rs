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

pub struct MyApp {
    config: AppConfig,
    config_dirty: bool,
    video_queue: Vec<PathBuf>,
    ffmpeg_log: Arc<Mutex<Vec<String>>>,
    ffmpeg_busy: Arc<AtomicBool>,
    tx: Option<Sender<String>>,
    should_start_next: Arc<Mutex<bool>>,
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
            should_start_next: Arc::new(Mutex::new(false)),
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

        let target_size = self.config.target_size_mb * 1000;

        // Spawn thread to run FFmpeg
        thread::spawn(move || {
            let output_path = path.with_extension("compressed.mp4");

            // calculate desired bitrate
            let Some((video_bitrate, audio_bitrate)) = calculate_bitrate(path.to_str().unwrap(), target_size) else { todo!() };
            println!("Target size: {}, Video bitrate: {}, Audio bitrate: {}", target_size, video_bitrate, audio_bitrate);

            let mut cmd = Command::new("ffmpeg")
                .args([
                    "-i", path.to_str().unwrap(),
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

        // Clone log and busy flag for the log reading thread
        let log_arc = Arc::clone(&self.ffmpeg_log);
        let busy_flag = Arc::clone(&self.ffmpeg_busy);
        let should_start_next_clone = Arc::clone(&self.should_start_next);

        thread::spawn(move || {
            while let Ok(line) = log_rx.recv() {
                if line == "[done]" {
                    busy_flag.store(false, Ordering::SeqCst);
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
        if !self.ffmpeg_busy.load(Ordering::SeqCst) && !self.video_queue.is_empty() {
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
