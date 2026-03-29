//! Shared top-level error type used by the CLI.

use std::io;

use thiserror::Error;

use crate::{config::ConfigError, ffmpeg::StreamError, mqtt::MqttError, output::OutputError};

#[derive(Debug, Error)]
pub enum AppError {
    // `transparent` keeps the original error message so callers get the most useful details.
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Stream(#[from] StreamError),
    #[error(transparent)]
    Mqtt(#[from] MqttError),
    #[error(transparent)]
    Output(#[from] OutputError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    CtrlC(#[from] ctrlc::Error),
    #[error("processing thread failed")]
    ProcessingThread,
    #[error("detection stream thread failed")]
    DetectionStreamThread,
    #[error("output stream thread failed")]
    OutputStreamThread,
    #[error("mqtt publish thread failed")]
    MqttPublishThread,
    #[error("mqtt event thread failed")]
    MqttEventThread,
    #[error("rtsp retry limit reached after {retries} retries")]
    RetryLimitReached { retries: u32 },
}
