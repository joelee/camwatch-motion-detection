//! Application bootstrap and thread coordination.
//!
//! This module ties the other small modules together: load config, start MQTT, stream frames,
//! detect motion, and shut everything down cleanly.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Serialize;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::{
    config::{AppConfig, InputSource, MotionDetectionConfig},
    error::AppError,
    ffmpeg::{VideoFrame, stream_input},
    motion::{MotionDetector, encode_snapshot_jpeg},
    mqtt::{self, MqttRuntimeError},
};

pub fn run() -> Result<(), AppError> {
    init_tracing();

    let config = AppConfig::load()?;
    let shutdown = Arc::new(AtomicBool::new(false));
    install_signal_handler(shutdown.clone())?;

    info!(
        input = %config.input.display_value(),
        config = %config.config_path.display(),
        "starting camwatch motion detection"
    );

    let mqtt_runtime = mqtt::start(&config.motion_detection)?;
    let mqtt_sender = mqtt_runtime.sender();
    // A tiny bounded channel provides backpressure so frame ingestion does not run far ahead of
    // motion analysis and waste memory.
    let (frame_sender, frame_receiver) = mpsc::sync_channel::<VideoFrame>(2);

    let processor = spawn_processor(
        config.input.clone(),
        config.motion_detection.clone(),
        frame_receiver,
        mqtt_sender,
    );

    let stream_result = stream_with_retries(
        &config.input,
        &config.motion_detection,
        &frame_sender,
        &shutdown,
    );

    drop(frame_sender);

    processor.join().map_err(|_| AppError::ProcessingThread)?;
    mqtt_runtime.shutdown().map_err(map_mqtt_runtime_error)?;
    stream_result
}

fn map_mqtt_runtime_error(error: MqttRuntimeError) -> AppError {
    match error {
        MqttRuntimeError::PublishThread => AppError::MqttPublishThread,
        MqttRuntimeError::EventThread => AppError::MqttEventThread,
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

fn install_signal_handler(shutdown: Arc<AtomicBool>) -> Result<(), AppError> {
    ctrlc::set_handler(move || {
        // The signal handler only flips a flag because more complex work is not signal-safe.
        shutdown.store(true, Ordering::Relaxed);
        warn!("shutdown requested; finishing current work");
    })?;
    Ok(())
}

fn spawn_processor(
    input: InputSource,
    settings: MotionDetectionConfig,
    frame_receiver: mpsc::Receiver<VideoFrame>,
    mqtt_sender: mpsc::Sender<Vec<u8>>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("motion-processor".to_owned())
        .spawn(move || {
            let mut detector = MotionDetector::new(&settings);
            for frame in frame_receiver {
                if let Some(event) = detector.analyze(&frame) {
                    // Build the payload on the same thread that made the detection decision so the
                    // JPEG snapshot always matches the triggering frame.
                    match build_motion_payload(
                        &input,
                        &frame,
                        event.motion_ratio,
                        settings.snapshot_jpeg_quality,
                    ) {
                        Ok(payload) => {
                            if let Err(error) = mqtt_sender.send(payload) {
                                error!(?error, "failed to enqueue mqtt payload");
                                break;
                            }
                            info!(
                                frame_index = frame.index,
                                motion_ratio = event.motion_ratio,
                                local_motion_ratio = event.local_motion_ratio,
                                "motion detected"
                            );
                        }
                        Err(error) => error!(?error, "failed to build motion event payload"),
                    }
                }
            }
        })
        .unwrap_or_else(|error| panic!("failed to spawn processor thread: {error}"))
}

fn stream_with_retries(
    input: &InputSource,
    settings: &MotionDetectionConfig,
    frame_sender: &SyncSender<VideoFrame>,
    shutdown: &Arc<AtomicBool>,
) -> Result<(), AppError> {
    if !input.is_rtsp() {
        return stream_input(input, settings, frame_sender, shutdown).map_err(AppError::from);
    }

    // RTSP sources are expected to be long-lived, so we loop and reconnect when the stream drops.
    let mut retries = 0_u32;
    loop {
        if shutdown.load(Ordering::Relaxed) {
            return Ok(());
        }

        match stream_input(input, settings, frame_sender, shutdown) {
            Ok(()) if shutdown.load(Ordering::Relaxed) => return Ok(()),
            Ok(()) => warn!("rtsp stream ended; attempting reconnect"),
            Err(error) => warn!(?error, "rtsp stream disconnected"),
        }

        if retries >= settings.rtsp_max_retries {
            return Err(AppError::RetryLimitReached { retries });
        }

        retries = retries.saturating_add(1);
        info!(
            retry = retries,
            delay_seconds = settings.rtsp_retry_delay_seconds,
            "waiting before rtsp reconnect"
        );
        sleep_with_shutdown(settings.rtsp_retry_delay_seconds, shutdown);
    }
}

fn sleep_with_shutdown(seconds: u64, shutdown: &Arc<AtomicBool>) {
    for _ in 0..seconds {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn build_motion_payload(
    input: &InputSource,
    frame: &VideoFrame,
    motion_ratio: f32,
    snapshot_quality: u8,
) -> Result<Vec<u8>, PayloadError> {
    let snapshot = encode_snapshot_jpeg(frame, snapshot_quality)?;
    // Base64 keeps the payload plain JSON, which is convenient for simple subscribers.
    let payload = MotionPayload {
        source: input.display_value(),
        captured_at_epoch_ms: system_time_to_epoch_ms(frame.captured_at),
        frame_index: frame.index,
        motion_ratio,
        frame_width: frame.width,
        frame_height: frame.height,
        snapshot_jpeg_base64: STANDARD.encode(snapshot),
    };

    serde_json::to_vec(&payload).map_err(PayloadError::Serialize)
}

fn system_time_to_epoch_ms(time: SystemTime) -> u128 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis(),
        Err(_) => 0,
    }
}

#[derive(Debug, Serialize)]
struct MotionPayload {
    source: String,
    captured_at_epoch_ms: u128,
    frame_index: u64,
    motion_ratio: f32,
    frame_width: u32,
    frame_height: u32,
    snapshot_jpeg_base64: String,
}

#[derive(Debug, thiserror::Error)]
enum PayloadError {
    #[error(transparent)]
    Snapshot(#[from] image::ImageError),
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use serde_json::Value;

    use super::{build_motion_payload, system_time_to_epoch_ms};
    use crate::{config::InputSource, ffmpeg::VideoFrame};

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
    fn motion_payload_contains_snapshot_and_metadata() {
        let payload = match build_motion_payload(
            &InputSource::Rtsp("rtsp://camera/live".to_owned()),
            &build_frame(),
            0.42,
            80,
        ) {
            Ok(payload) => payload,
            Err(error) => panic!("expected payload to build, got {error}"),
        };

        let json: Value = match serde_json::from_slice(&payload) {
            Ok(value) => value,
            Err(error) => panic!("expected valid json payload, got {error}"),
        };

        assert_eq!(json["frame_index"], 7);
        assert_eq!(json["captured_at_epoch_ms"], 0);
        assert!(json["snapshot_jpeg_base64"].as_str().is_some());
    }

    #[test]
    fn unix_epoch_is_zero_ms() {
        assert_eq!(system_time_to_epoch_ms(SystemTime::UNIX_EPOCH), 0);
    }
}
