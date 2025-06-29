use serde::{Serialize, Deserialize};

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

// resolution scaling
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum Resolution {
    R1080,
    R720,
    R480,
}

impl Resolution {
    pub fn to_height(&self) -> u32 {
        match self {
            Resolution::R1080 => 1080,
            Resolution::R720 => 720,
            Resolution::R480 => 480,
        }
    }
}

impl std::fmt::Display for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Resolution::R1080 => write!(f, "1080p"),
            Resolution::R720 => write!(f, "720p"),
            Resolution::R480 => write!(f, "480p"),
        }
    }
}

// https://trac.ffmpeg.org/wiki/Encode/H.264#Preset
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum Preset {
    None,
    Ultrafast,
    Superfast,
    Veryfast,
    Faster,
    Fast,
    Medium,
    Slow,
    Slower,
    Veryslow,
}

impl Default for Preset {
    fn default() -> Self {
        Preset::None
    }
}

impl Preset {
    pub fn as_str(&self) -> Option<&'static str> {
        match self {
            Preset::None => None,
            Preset::Ultrafast => Some("ultrafast"),
            Preset::Superfast => Some("superfast"),
            Preset::Veryfast => Some("veryfast"),
            Preset::Faster => Some("faster"),
            Preset::Fast => Some("fast"),
            Preset::Medium => Some("medium"),
            Preset::Slow => Some("slow"),
            Preset::Slower => Some("slower"),
            Preset::Veryslow => Some("veryslow"),
        }
    }
}