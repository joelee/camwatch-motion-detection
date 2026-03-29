//! Command-line and TOML configuration loading.
//!
//! New Rust users often find it easier to follow when configuration is separated from the
//! processing code, so this module owns parsing, defaults, and validation.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use clap::Parser;
use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Detect motion on RTSP streams and video files"
)]
pub struct Cli {
    #[arg(long, short = 'i', help = "RTSP URL or video file path")]
    pub input: String,

    #[arg(long, help = "Path to TOML configuration")]
    pub config: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputSource {
    Rtsp(String),
    File(PathBuf),
}

impl InputSource {
    pub fn parse(raw: &str) -> Result<Self, ConfigError> {
        // RTSP inputs are treated as URLs; everything else is expected to be a local file.
        if raw.starts_with("rtsp://") || raw.starts_with("rtsps://") {
            return Ok(Self::Rtsp(raw.to_owned()));
        }

        let path = PathBuf::from(raw);
        if !path.exists() {
            return Err(ConfigError::InputFileNotFound(path));
        }

        Ok(Self::File(path))
    }

    pub fn display_value(&self) -> String {
        match self {
            Self::Rtsp(url) => url.clone(),
            Self::File(path) => path.display().to_string(),
        }
    }

    pub fn is_rtsp(&self) -> bool {
        matches!(self, Self::Rtsp(_))
    }
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub input: InputSource,
    pub config_path: PathBuf,
    pub motion_detection: MotionDetectionConfig,
}

impl AppConfig {
    pub fn load() -> Result<Self, ConfigError> {
        // Clap reads CLI flags from the process arguments and fills in the `Cli` struct.
        let cli = Cli::parse();
        Self::from_args(cli)
    }

    fn from_args(cli: Cli) -> Result<Self, ConfigError> {
        let input = InputSource::parse(&cli.input)?;
        let config_path = resolve_config_path(cli.config)?;
        let file = load_config_file(&config_path)?;
        let motion_detection = file.motion_detection.validate()?;

        Ok(Self {
            input,
            config_path,
            motion_detection,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
struct CamwatchConfigFile {
    #[serde(default)]
    motion_detection: MotionDetectionConfig,
}

fn load_config_file(path: &Path) -> Result<CamwatchConfigFile, ConfigError> {
    let contents = fs::read_to_string(path).map_err(|source| ConfigError::ReadConfigFile {
        path: path.into(),
        source,
    })?;
    toml::from_str(&contents).map_err(|source| ConfigError::ParseConfigFile {
        path: path.into(),
        source,
    })
}

fn resolve_config_path(cli_path: Option<PathBuf>) -> Result<PathBuf, ConfigError> {
    let home_dir = env::var_os("HOME").map(PathBuf::from);
    resolve_config_path_with_home(cli_path, home_dir)
}

fn resolve_config_path_with_home(
    cli_path: Option<PathBuf>,
    home_dir: Option<PathBuf>,
) -> Result<PathBuf, ConfigError> {
    if let Some(path) = cli_path {
        return Ok(path);
    }

    let candidates = default_config_candidates(home_dir);
    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }

    Err(ConfigError::DefaultConfigNotFound {
        searched: candidates,
    })
}

fn default_config_candidates(home_dir: Option<PathBuf>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(home_dir) = home_dir {
        candidates.push(home_dir.join(".config/camwatch/camwatch.toml"));
        candidates.push(home_dir.join(".config/camwatch.toml"));
    }

    candidates.push(PathBuf::from("/etc/camwatch.toml"));
    candidates
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RtspTransport {
    #[default]
    Tcp,
    Udp,
}

impl RtspTransport {
    pub fn as_ffmpeg_value(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct MotionDetectionConfig {
    pub frame_width: u32,
    pub frame_height: u32,
    pub output_frame_width: Option<u32>,
    pub output_frame_height: Option<u32>,
    pub frame_rate: u32,
    pub pixel_difference_threshold: u8,
    pub motion_ratio_threshold: f32,
    pub local_motion_ratio_threshold: f32,
    pub local_motion_consecutive_frames: u32,
    pub motion_end_grace_seconds: u64,
    pub motion_snapshot_delay_seconds: u64,
    pub long_motion_snapshot_interval_seconds: u64,
    pub background_alpha: f32,
    pub event_cooldown_seconds: u64,
    pub snapshot_jpeg_quality: u8,
    pub output_directory: Option<PathBuf>,
    pub mqtt_host: String,
    pub mqtt_port: u16,
    pub mqtt_topic: String,
    pub mqtt_client_id: String,
    pub mqtt_username: Option<String>,
    pub mqtt_password: Option<String>,
    pub mqtt_qos: u8,
    pub mqtt_keep_alive_seconds: u64,
    pub rtsp_transport: RtspTransport,
    pub rtsp_retry_delay_seconds: u64,
    pub rtsp_max_retries: u32,
}

impl Default for MotionDetectionConfig {
    fn default() -> Self {
        Self {
            frame_width: 320,
            frame_height: 180,
            output_frame_width: None,
            output_frame_height: None,
            frame_rate: 5,
            pixel_difference_threshold: 20,
            motion_ratio_threshold: 0.015,
            local_motion_ratio_threshold: 0.095,
            local_motion_consecutive_frames: 4,
            motion_end_grace_seconds: 1,
            motion_snapshot_delay_seconds: 5,
            long_motion_snapshot_interval_seconds: 30,
            background_alpha: 0.08,
            event_cooldown_seconds: 10,
            snapshot_jpeg_quality: 80,
            output_directory: None,
            mqtt_host: "127.0.0.1".to_owned(),
            mqtt_port: 1883,
            mqtt_topic: "camwatch/motion".to_owned(),
            mqtt_client_id: "camwatch-motion-detection".to_owned(),
            mqtt_username: None,
            mqtt_password: None,
            mqtt_qos: 1,
            mqtt_keep_alive_seconds: 30,
            rtsp_transport: RtspTransport::Tcp,
            rtsp_retry_delay_seconds: 5,
            rtsp_max_retries: 12,
        }
    }
}

impl MotionDetectionConfig {
    fn validate(self) -> Result<Self, ConfigError> {
        let mut config = self;
        let output_directory = config
            .output_directory
            .take()
            .filter(|path| !path.as_os_str().is_empty());
        let mqtt_host_set = !config.mqtt_host.trim().is_empty();
        let mqtt_topic_set = !config.mqtt_topic.trim().is_empty();
        let mqtt_enabled = mqtt_host_set && mqtt_topic_set;

        // Validation happens once at startup so the rest of the app can assume sane values.
        if config.frame_width == 0 {
            return Err(ConfigError::InvalidValue(
                "frame_width must be greater than 0",
            ));
        }
        if config.frame_height == 0 {
            return Err(ConfigError::InvalidValue(
                "frame_height must be greater than 0",
            ));
        }
        if config.frame_rate == 0 {
            return Err(ConfigError::InvalidValue(
                "frame_rate must be greater than 0",
            ));
        }
        if config.output_frame_width.is_some() != config.output_frame_height.is_some() {
            return Err(ConfigError::InvalidValue(
                "output_frame_width and output_frame_height must either both be set or both be omitted",
            ));
        }
        if let Some(output_frame_width) = config.output_frame_width
            && output_frame_width == 0
        {
            return Err(ConfigError::InvalidValue(
                "output_frame_width must be greater than 0",
            ));
        }
        if let Some(output_frame_height) = config.output_frame_height
            && output_frame_height == 0
        {
            return Err(ConfigError::InvalidValue(
                "output_frame_height must be greater than 0",
            ));
        }
        if !(0.0..=1.0).contains(&config.motion_ratio_threshold)
            || config.motion_ratio_threshold == 0.0
        {
            return Err(ConfigError::InvalidValue(
                "motion_ratio_threshold must be between 0.0 and 1.0",
            ));
        }
        if !(0.0..=1.0).contains(&config.local_motion_ratio_threshold)
            || config.local_motion_ratio_threshold == 0.0
        {
            return Err(ConfigError::InvalidValue(
                "local_motion_ratio_threshold must be between 0.0 and 1.0",
            ));
        }
        if config.local_motion_consecutive_frames == 0 {
            return Err(ConfigError::InvalidValue(
                "local_motion_consecutive_frames must be greater than 0",
            ));
        }
        if config.motion_end_grace_seconds == 0 {
            return Err(ConfigError::InvalidValue(
                "motion_end_grace_seconds must be greater than 0",
            ));
        }
        if config.motion_snapshot_delay_seconds == 0 {
            return Err(ConfigError::InvalidValue(
                "motion_snapshot_delay_seconds must be greater than 0",
            ));
        }
        if config.long_motion_snapshot_interval_seconds == 0 {
            return Err(ConfigError::InvalidValue(
                "long_motion_snapshot_interval_seconds must be greater than 0",
            ));
        }
        if config.long_motion_snapshot_interval_seconds <= config.motion_snapshot_delay_seconds {
            return Err(ConfigError::InvalidValue(
                "long_motion_snapshot_interval_seconds must be greater than motion_snapshot_delay_seconds",
            ));
        }
        if !(0.0..=1.0).contains(&config.background_alpha) || config.background_alpha == 0.0 {
            return Err(ConfigError::InvalidValue(
                "background_alpha must be between 0.0 and 1.0",
            ));
        }
        if config.snapshot_jpeg_quality == 0 || config.snapshot_jpeg_quality > 100 {
            return Err(ConfigError::InvalidValue(
                "snapshot_jpeg_quality must be between 1 and 100",
            ));
        }
        if mqtt_host_set != mqtt_topic_set {
            return Err(ConfigError::InvalidValue(
                "mqtt_host and mqtt_topic must either both be set or both be empty",
            ));
        }
        if mqtt_enabled && config.mqtt_client_id.trim().is_empty() {
            return Err(ConfigError::InvalidValue(
                "mqtt_client_id must not be empty",
            ));
        }
        if config.mqtt_qos > 2 {
            return Err(ConfigError::InvalidValue("mqtt_qos must be 0, 1, or 2"));
        }
        if mqtt_enabled && config.mqtt_keep_alive_seconds == 0 {
            return Err(ConfigError::InvalidValue(
                "mqtt_keep_alive_seconds must be greater than 0",
            ));
        }
        if !mqtt_enabled && output_directory.is_none() {
            return Err(ConfigError::InvalidValue(
                "set mqtt_host and mqtt_topic, output_directory, or both",
            ));
        }
        if config.rtsp_retry_delay_seconds == 0 {
            return Err(ConfigError::InvalidValue(
                "rtsp_retry_delay_seconds must be greater than 0",
            ));
        }

        config.output_directory = output_directory;
        Ok(config)
    }

    pub fn mqtt_enabled(&self) -> bool {
        !self.mqtt_host.trim().is_empty() && !self.mqtt_topic.trim().is_empty()
    }

    pub fn output_directory_enabled(&self) -> bool {
        self.output_directory.is_some()
    }

    pub fn configured_output_dimensions(&self) -> Option<(u32, u32)> {
        match (self.output_frame_width, self.output_frame_height) {
            (Some(width), Some(height)) => Some((width, height)),
            _ => None,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("input file not found: {0}")]
    InputFileNotFound(PathBuf),
    #[error("failed to read config file {path}: {source}")]
    ReadConfigFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse config file {path}: {source}")]
    ParseConfigFile {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("invalid configuration: {0}")]
    InvalidValue(&'static str),
    #[error(
        "no default config file found; searched: {searched}",
        searched = format_searched_paths(.searched)
    )]
    DefaultConfigNotFound { searched: Vec<PathBuf> },
}

fn format_searched_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use tempfile::tempdir;

    use super::{
        Cli, ConfigError, InputSource, MotionDetectionConfig, RtspTransport,
        resolve_config_path_with_home,
    };

    fn load_with_paths(config_path: &Path, input_path: &Path) -> super::AppConfig {
        let cli = Cli {
            input: input_path.display().to_string(),
            config: Some(config_path.to_path_buf()),
        };

        match super::AppConfig::from_args(cli) {
            Ok(config) => config,
            Err(error) => panic!("expected valid config, got {error}"),
        }
    }

    #[test]
    fn parses_file_input() {
        let temp_dir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create temp dir: {error}"),
        };
        let input_path = temp_dir.path().join("example.mp4");
        if let Err(error) = fs::write(&input_path, b"placeholder") {
            panic!("failed to create input file: {error}");
        }

        let input = match InputSource::parse(&input_path.display().to_string()) {
            Ok(source) => source,
            Err(error) => panic!("expected file input to parse, got {error}"),
        };

        assert!(matches!(input, InputSource::File(_)));
    }

    #[test]
    fn parses_rtsp_input() {
        let input = match InputSource::parse("rtsp://camera.local/live") {
            Ok(source) => source,
            Err(error) => panic!("expected rtsp input to parse, got {error}"),
        };

        assert!(matches!(input, InputSource::Rtsp(_)));
    }

    #[test]
    fn loads_defaults_from_minimal_file() {
        let temp_dir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create temp dir: {error}"),
        };
        let config_path = temp_dir.path().join("camwatch.toml");
        let input_path = temp_dir.path().join("input.mp4");

        if let Err(error) = fs::write(&config_path, "[motion_detection]\n") {
            panic!("failed to write config: {error}");
        }
        if let Err(error) = fs::write(&input_path, b"placeholder") {
            panic!("failed to write input: {error}");
        }

        let config = load_with_paths(&config_path, &input_path);

        assert_eq!(
            config.motion_detection.frame_width,
            MotionDetectionConfig::default().frame_width
        );
        assert_eq!(config.motion_detection.rtsp_transport, RtspTransport::Tcp);
    }

    #[test]
    fn loads_custom_values_from_toml() {
        let temp_dir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create temp dir: {error}"),
        };
        let config_path = temp_dir.path().join("camwatch.toml");
        let input_path = temp_dir.path().join("input.mp4");

        let contents = r#"
[motion_detection]
frame_width = 640
frame_height = 360
output_frame_width = 1280
output_frame_height = 720
frame_rate = 3
mqtt_topic = "custom/topic"
local_motion_ratio_threshold = 0.2
local_motion_consecutive_frames = 2
motion_end_grace_seconds = 2
motion_snapshot_delay_seconds = 4
long_motion_snapshot_interval_seconds = 20
rtsp_transport = "udp"
rtsp_retry_delay_seconds = 9
rtsp_max_retries = 2
"#;

        if let Err(error) = fs::write(&config_path, contents) {
            panic!("failed to write config: {error}");
        }
        if let Err(error) = fs::write(&input_path, b"placeholder") {
            panic!("failed to write input: {error}");
        }

        let config = load_with_paths(&config_path, &input_path);

        assert_eq!(config.motion_detection.frame_width, 640);
        assert_eq!(config.motion_detection.frame_height, 360);
        assert_eq!(config.motion_detection.output_frame_width, Some(1280));
        assert_eq!(config.motion_detection.output_frame_height, Some(720));
        assert_eq!(config.motion_detection.frame_rate, 3);
        assert_eq!(config.motion_detection.mqtt_topic, "custom/topic");
        assert_eq!(config.motion_detection.local_motion_ratio_threshold, 0.2);
        assert_eq!(config.motion_detection.local_motion_consecutive_frames, 2);
        assert_eq!(config.motion_detection.motion_end_grace_seconds, 2);
        assert_eq!(config.motion_detection.motion_snapshot_delay_seconds, 4);
        assert_eq!(
            config
                .motion_detection
                .long_motion_snapshot_interval_seconds,
            20
        );
        assert_eq!(config.motion_detection.rtsp_transport, RtspTransport::Udp);
        assert_eq!(config.motion_detection.rtsp_retry_delay_seconds, 9);
        assert_eq!(config.motion_detection.rtsp_max_retries, 2);
    }

    #[test]
    fn rejects_invalid_qos() {
        let config = MotionDetectionConfig {
            mqtt_qos: 3,
            ..MotionDetectionConfig::default()
        };

        let result = config.validate();

        assert!(result.is_err());
    }

    #[test]
    fn rejects_partial_output_dimensions() {
        let config = MotionDetectionConfig {
            output_frame_width: Some(640),
            output_frame_height: None,
            ..MotionDetectionConfig::default()
        };

        let result = config.validate();

        assert!(result.is_err());
    }

    #[test]
    fn allows_file_output_without_mqtt() {
        let config = MotionDetectionConfig {
            output_directory: Some("/tmp/camwatch-output".into()),
            mqtt_host: String::new(),
            mqtt_topic: String::new(),
            ..MotionDetectionConfig::default()
        };

        let validated = config
            .validate()
            .unwrap_or_else(|error| panic!("expected file output config to validate, got {error}"));

        assert!(!validated.mqtt_enabled());
        assert!(validated.output_directory_enabled());
    }

    #[test]
    fn rejects_config_without_any_output_target() {
        let config = MotionDetectionConfig {
            mqtt_host: String::new(),
            mqtt_topic: String::new(),
            output_directory: None,
            ..MotionDetectionConfig::default()
        };

        let result = config.validate();

        assert!(result.is_err());
    }

    #[test]
    fn rejects_partially_configured_mqtt() {
        let config = MotionDetectionConfig {
            mqtt_topic: String::new(),
            ..MotionDetectionConfig::default()
        };

        let result = config.validate();

        assert!(result.is_err());
    }

    #[test]
    fn prefers_nested_home_config_when_cli_flag_is_missing() {
        let temp_dir =
            tempdir().unwrap_or_else(|error| panic!("failed to create temp dir: {error}"));
        let nested = temp_dir.path().join(".config/camwatch/camwatch.toml");
        let parent = nested
            .parent()
            .unwrap_or_else(|| panic!("expected parent directory"));

        fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("failed to create config dir: {error}"));
        fs::write(&nested, "[motion_detection]\n")
            .unwrap_or_else(|error| panic!("failed to write nested config: {error}"));

        let resolved = resolve_config_path_with_home(None, Some(temp_dir.path().to_path_buf()))
            .unwrap_or_else(|error| panic!("expected config to resolve, got {error}"));

        assert_eq!(resolved, nested);
    }

    #[test]
    fn falls_back_to_flat_home_config_when_nested_config_is_missing() {
        let temp_dir =
            tempdir().unwrap_or_else(|error| panic!("failed to create temp dir: {error}"));
        let flat = temp_dir.path().join(".config/camwatch.toml");
        let parent = flat
            .parent()
            .unwrap_or_else(|| panic!("expected parent directory"));

        fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("failed to create config dir: {error}"));
        fs::write(&flat, "[motion_detection]\n")
            .unwrap_or_else(|error| panic!("failed to write flat config: {error}"));

        let resolved = resolve_config_path_with_home(None, Some(temp_dir.path().to_path_buf()))
            .unwrap_or_else(|error| panic!("expected config to resolve, got {error}"));

        assert_eq!(resolved, flat);
    }

    #[test]
    fn returns_error_when_no_default_config_exists() {
        let temp_dir =
            tempdir().unwrap_or_else(|error| panic!("failed to create temp dir: {error}"));

        let result = resolve_config_path_with_home(None, Some(temp_dir.path().to_path_buf()));

        assert!(matches!(
            result,
            Err(ConfigError::DefaultConfigNotFound { .. })
        ));
    }
}
