use std::path::{PathBuf};
use serde::{Serialize, Deserialize};

use crate::types::compression::{Encoder, Preset, Resolution};

// compression options
#[derive(Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_target_size")]
    pub target_size_mb: u32,

    pub frame_rate: Option<u32>,
    
    #[serde(default)]
    pub encoder: Encoder,

    #[serde(default)]
    pub dark_mode_enabled: bool,

    pub resolution: Option<Resolution>,

    #[serde(default)]
    pub preset: Preset,
}

fn default_target_size() -> u32 {
    10
}

impl ::std::default::Default for AppConfig {
    fn default() -> Self {
        Self {
            target_size_mb: 10,
            frame_rate: None,
            encoder: Encoder::CpuX264,
            dark_mode_enabled: false,
            resolution: None,
            preset: Preset::None,
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
    pub output_size_bytes: Option<u64>,
}