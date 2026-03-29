//! Motion event outputs shared by MQTT and file writing.
//!
//! Keeping output formatting in one place helps the application send the same capture metadata to
//! multiple destinations without duplicating serialization logic.

use std::{fs, io, path::PathBuf, time::SystemTime};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use serde::Serialize;
use thiserror::Error;

use crate::{config::InputSource, motion::encode_snapshot_jpeg, session::MotionSessionCapture};

#[derive(Debug, Clone)]
pub struct MotionCapture {
    pub source: String,
    pub captured_at: SystemTime,
    pub captured_at_epoch_ms: u64,
    pub motion_started_at: SystemTime,
    pub motion_started_at_epoch_ms: u64,
    pub motion_ended_at: SystemTime,
    pub motion_ended_at_epoch_ms: u64,
    pub motion_duration_ms: u64,
    pub frame_index: u64,
    pub motion_started_frame_index: u64,
    pub motion_ended_frame_index: u64,
    pub motion_ratio: f32,
    pub local_motion_ratio: f32,
    pub frame_width: u32,
    pub frame_height: u32,
    pub snapshot_jpeg: Vec<u8>,
}

impl MotionCapture {
    pub fn from_session_capture(
        input: &InputSource,
        capture: &MotionSessionCapture,
        snapshot_quality: u8,
    ) -> Result<Self, OutputError> {
        let motion_started_at_epoch_ms = system_time_to_epoch_ms(capture.motion_started_at);
        let motion_ended_at_epoch_ms = system_time_to_epoch_ms(capture.motion_ended_at);

        Ok(Self {
            source: input.display_value(),
            captured_at: capture.frame.captured_at,
            captured_at_epoch_ms: system_time_to_epoch_ms(capture.frame.captured_at),
            motion_started_at: capture.motion_started_at,
            motion_started_at_epoch_ms,
            motion_ended_at: capture.motion_ended_at,
            motion_ended_at_epoch_ms,
            motion_duration_ms: motion_ended_at_epoch_ms.saturating_sub(motion_started_at_epoch_ms),
            frame_index: capture.frame.index,
            motion_started_frame_index: capture.motion_started_frame_index,
            motion_ended_frame_index: capture.motion_ended_frame_index,
            motion_ratio: capture.event.motion_ratio,
            local_motion_ratio: capture.event.local_motion_ratio,
            frame_width: capture.frame.width,
            frame_height: capture.frame.height,
            snapshot_jpeg: encode_snapshot_jpeg(&capture.frame, snapshot_quality)?,
        })
    }

    pub fn mqtt_payload(&self) -> Result<Vec<u8>, OutputError> {
        let payload = MqttMotionPayload {
            source: self.source.clone(),
            captured_at_epoch_ms: self.captured_at_epoch_ms,
            motion_started_at_epoch_ms: self.motion_started_at_epoch_ms,
            motion_ended_at_epoch_ms: self.motion_ended_at_epoch_ms,
            motion_duration_ms: self.motion_duration_ms,
            frame_index: self.frame_index,
            motion_started_frame_index: self.motion_started_frame_index,
            motion_ended_frame_index: self.motion_ended_frame_index,
            motion_ratio: self.motion_ratio,
            local_motion_ratio: self.local_motion_ratio,
            frame_width: self.frame_width,
            frame_height: self.frame_height,
            snapshot_jpeg_base64: STANDARD.encode(&self.snapshot_jpeg),
        };

        serde_json::to_vec(&payload).map_err(OutputError::SerializeJson)
    }

    pub fn file_stem(&self) -> String {
        let captured_at: DateTime<Utc> = self.captured_at.into();
        format!("motion-{}", captured_at.format("%Y%m%d-%H%M%S"))
    }
}

pub struct FileOutputWriter {
    directory: PathBuf,
}

impl FileOutputWriter {
    pub fn new(directory: PathBuf) -> Result<Self, OutputError> {
        fs::create_dir_all(&directory).map_err(|source| OutputError::CreateOutputDirectory {
            path: directory.clone(),
            source,
        })?;

        Ok(Self { directory })
    }

    pub fn write_capture(&self, capture: &MotionCapture) -> Result<FileOutputPaths, OutputError> {
        let stem = capture.file_stem();
        let image_filename = format!("{stem}.jpg");
        let metadata_filename = format!("{stem}.toml");
        let image_path = self.directory.join(&image_filename);
        let metadata_path = self.directory.join(&metadata_filename);

        fs::write(&image_path, &capture.snapshot_jpeg).map_err(|source| {
            OutputError::WriteFile {
                path: image_path.clone(),
                source,
            }
        })?;

        let metadata = FileMotionMetadata {
            source: capture.source.clone(),
            captured_at_epoch_ms: capture.captured_at_epoch_ms,
            captured_at_rfc3339: DateTime::<Utc>::from(capture.captured_at).to_rfc3339(),
            motion_started_at_epoch_ms: capture.motion_started_at_epoch_ms,
            motion_started_at_rfc3339: DateTime::<Utc>::from(capture.motion_started_at)
                .to_rfc3339(),
            motion_ended_at_epoch_ms: capture.motion_ended_at_epoch_ms,
            motion_ended_at_rfc3339: DateTime::<Utc>::from(capture.motion_ended_at).to_rfc3339(),
            motion_duration_ms: capture.motion_duration_ms,
            frame_index: capture.frame_index,
            motion_started_frame_index: capture.motion_started_frame_index,
            motion_ended_frame_index: capture.motion_ended_frame_index,
            motion_ratio: capture.motion_ratio,
            local_motion_ratio: capture.local_motion_ratio,
            frame_width: capture.frame_width,
            frame_height: capture.frame_height,
            snapshot_file: image_filename,
        };
        let metadata_toml =
            toml::to_string_pretty(&metadata).map_err(OutputError::SerializeToml)?;

        fs::write(&metadata_path, metadata_toml).map_err(|source| OutputError::WriteFile {
            path: metadata_path.clone(),
            source,
        })?;

        Ok(FileOutputPaths {
            image_path,
            metadata_path,
        })
    }
}

pub struct FileOutputPaths {
    pub image_path: PathBuf,
    pub metadata_path: PathBuf,
}

#[derive(Debug, Serialize)]
struct MqttMotionPayload {
    source: String,
    captured_at_epoch_ms: u64,
    motion_started_at_epoch_ms: u64,
    motion_ended_at_epoch_ms: u64,
    motion_duration_ms: u64,
    frame_index: u64,
    motion_started_frame_index: u64,
    motion_ended_frame_index: u64,
    motion_ratio: f32,
    local_motion_ratio: f32,
    frame_width: u32,
    frame_height: u32,
    snapshot_jpeg_base64: String,
}

#[derive(Debug, Serialize)]
struct FileMotionMetadata {
    source: String,
    captured_at_epoch_ms: u64,
    captured_at_rfc3339: String,
    motion_started_at_epoch_ms: u64,
    motion_started_at_rfc3339: String,
    motion_ended_at_epoch_ms: u64,
    motion_ended_at_rfc3339: String,
    motion_duration_ms: u64,
    frame_index: u64,
    motion_started_frame_index: u64,
    motion_ended_frame_index: u64,
    motion_ratio: f32,
    local_motion_ratio: f32,
    frame_width: u32,
    frame_height: u32,
    snapshot_file: String,
}

#[derive(Debug, Error)]
pub enum OutputError {
    #[error(transparent)]
    Snapshot(#[from] image::ImageError),
    #[error(transparent)]
    SerializeJson(#[from] serde_json::Error),
    #[error(transparent)]
    SerializeToml(#[from] toml::ser::Error),
    #[error("failed to create output directory {path}: {source}")]
    CreateOutputDirectory { path: PathBuf, source: io::Error },
    #[error("failed to write file {path}: {source}")]
    WriteFile { path: PathBuf, source: io::Error },
}

fn system_time_to_epoch_ms(time: SystemTime) -> u64 {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, time::SystemTime};

    use serde_json::Value;
    use tempfile::tempdir;

    use super::{FileOutputWriter, MotionCapture};
    use crate::{
        config::InputSource, ffmpeg::VideoFrame, motion::MotionEvent, session::MotionSessionCapture,
    };

    fn build_frame() -> VideoFrame {
        VideoFrame {
            index: 7,
            captured_at: SystemTime::UNIX_EPOCH,
            width: 2,
            height: 2,
            rgb: vec![255, 255, 255, 0, 0, 0, 255, 255, 255, 0, 0, 0],
        }
    }

    #[test]
    fn mqtt_payload_contains_expected_fields() {
        let capture = MotionCapture::from_session_capture(
            &InputSource::Rtsp("rtsp://camera/live".to_owned()),
            &MotionSessionCapture {
                frame: build_frame(),
                event: MotionEvent {
                    motion_ratio: 0.42,
                    local_motion_ratio: 0.73,
                },
                motion_started_at: SystemTime::UNIX_EPOCH,
                motion_started_frame_index: 2,
                motion_ended_at: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(6),
                motion_ended_frame_index: 13,
            },
            80,
        )
        .unwrap_or_else(|error| panic!("expected capture to build, got {error}"));

        let payload = capture
            .mqtt_payload()
            .unwrap_or_else(|error| panic!("expected mqtt payload to serialize, got {error}"));
        let json: Value = serde_json::from_slice(&payload)
            .unwrap_or_else(|error| panic!("expected valid json payload, got {error}"));

        assert_eq!(json["frame_index"], 7);
        assert_eq!(json["captured_at_epoch_ms"], 0);
        assert_eq!(json["motion_duration_ms"], 6000);
        assert_eq!(json["local_motion_ratio"], 0.73);
        assert!(json["snapshot_jpeg_base64"].as_str().is_some());
    }

    #[test]
    fn file_writer_creates_jpeg_and_toml_files() {
        let temp_dir =
            tempdir().unwrap_or_else(|error| panic!("failed to create temp dir: {error}"));
        let writer = FileOutputWriter::new(temp_dir.path().to_path_buf())
            .unwrap_or_else(|error| panic!("expected writer to initialize, got {error}"));
        let capture = MotionCapture::from_session_capture(
            &InputSource::File("video.mp4".into()),
            &MotionSessionCapture {
                frame: build_frame(),
                event: MotionEvent {
                    motion_ratio: 0.42,
                    local_motion_ratio: 0.73,
                },
                motion_started_at: SystemTime::UNIX_EPOCH,
                motion_started_frame_index: 2,
                motion_ended_at: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(6),
                motion_ended_frame_index: 13,
            },
            80,
        )
        .unwrap_or_else(|error| panic!("expected capture to build, got {error}"));

        let paths = writer
            .write_capture(&capture)
            .unwrap_or_else(|error| panic!("expected capture files to be written, got {error}"));

        assert_eq!(
            paths
                .image_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_else(|| panic!("expected image filename")),
            "motion-19700101-000000.jpg"
        );
        assert_eq!(
            paths
                .metadata_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_else(|| panic!("expected metadata filename")),
            "motion-19700101-000000.toml"
        );

        let metadata = fs::read_to_string(&paths.metadata_path)
            .unwrap_or_else(|error| panic!("expected metadata file to be readable, got {error}"));

        assert!(paths.image_path.is_file());
        assert!(metadata.contains("snapshot_file = \"motion-19700101-000000.jpg\""));
        assert!(metadata.contains("local_motion_ratio = 0.73"));
        assert!(metadata.contains("motion_duration_ms = 6000"));
    }
}
