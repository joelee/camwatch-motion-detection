//! Application bootstrap and thread coordination.
//!
//! This module ties the other small modules together: load config, start MQTT, run the low-res
//! detection stream alongside the high-res snapshot stream, and shut everything down cleanly.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, SystemTime},
};

use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::{
    config::{AppConfig, InputSource, MotionDetectionConfig},
    error::AppError,
    ffmpeg::{FrameDimensions, VideoFrame, resolve_output_dimensions, stream_input},
    motion::{MotionDetector, MotionEvent},
    mqtt::{self, MqttRuntimeError},
    output::{FileOutputWriter, MotionCapture},
    session::{
        MotionSessionCapture, MotionSessionEvent, MotionSessionSummary, MotionSessionTracker,
        MotionSnapshotSelection,
    },
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

    let detection_dimensions = FrameDimensions {
        width: config.motion_detection.frame_width,
        height: config.motion_detection.frame_height,
    };
    let output_dimensions = resolve_output_dimensions(&config.input, &config.motion_detection)?;

    let mqtt_runtime = if config.motion_detection.mqtt_enabled() {
        Some(mqtt::start(&config.motion_detection)?)
    } else {
        None
    };
    let mqtt_sender = mqtt_runtime.as_ref().map(|runtime| runtime.sender());
    let file_writer = match config.motion_detection.output_directory.clone() {
        Some(directory) => Some(FileOutputWriter::new(directory)?),
        None => None,
    };

    let (stream_sender, stream_receiver) = mpsc::channel::<StreamMessage>();
    let processor = spawn_processor(
        config.input.clone(),
        config.motion_detection.clone(),
        stream_receiver,
        file_writer,
        mqtt_sender,
    );

    let detection_worker = spawn_stream_worker(
        StreamSource::Detection,
        config.input.clone(),
        config.motion_detection.clone(),
        detection_dimensions,
        stream_sender.clone(),
        shutdown.clone(),
    );
    let output_worker = spawn_stream_worker(
        StreamSource::Output,
        config.input.clone(),
        config.motion_detection.clone(),
        output_dimensions,
        stream_sender,
        shutdown.clone(),
    );

    let detection_result = detection_worker
        .join()
        .map_err(|_| AppError::DetectionStreamThread)?;
    let output_result = output_worker
        .join()
        .map_err(|_| AppError::OutputStreamThread)?;
    processor.join().map_err(|_| AppError::ProcessingThread)?;

    if let Some(mqtt_runtime) = mqtt_runtime {
        mqtt_runtime.shutdown().map_err(map_mqtt_runtime_error)?;
    }

    detection_result?;
    output_result?;
    Ok(())
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

fn spawn_stream_worker(
    source: StreamSource,
    input: InputSource,
    settings: MotionDetectionConfig,
    dimensions: FrameDimensions,
    sender: mpsc::Sender<StreamMessage>,
    shutdown: Arc<AtomicBool>,
) -> thread::JoinHandle<Result<(), AppError>> {
    thread::Builder::new()
        .name(format!("{:?}-stream", source).to_lowercase())
        .spawn(move || run_stream_worker(source, &input, &settings, dimensions, sender, &shutdown))
        .unwrap_or_else(|error| panic!("failed to spawn stream worker: {error}"))
}

fn run_stream_worker(
    source: StreamSource,
    input: &InputSource,
    settings: &MotionDetectionConfig,
    dimensions: FrameDimensions,
    sender: mpsc::Sender<StreamMessage>,
    shutdown: &Arc<AtomicBool>,
) -> Result<(), AppError> {
    if !input.is_rtsp() {
        return stream_once(source, input, settings, dimensions, &sender, shutdown);
    }

    let mut retries = 0_u32;
    loop {
        if shutdown.load(Ordering::Relaxed) {
            return Ok(());
        }

        match stream_once(source, input, settings, dimensions, &sender, shutdown) {
            Ok(()) if shutdown.load(Ordering::Relaxed) => return Ok(()),
            Ok(()) => warn!(stream = ?source, "rtsp stream ended; attempting reconnect"),
            Err(error) => warn!(stream = ?source, ?error, "rtsp stream disconnected"),
        }

        if retries >= settings.rtsp_max_retries {
            shutdown.store(true, Ordering::Relaxed);
            return Err(AppError::RetryLimitReached { retries });
        }

        retries = retries.saturating_add(1);
        info!(
            stream = ?source,
            retry = retries,
            delay_seconds = settings.rtsp_retry_delay_seconds,
            "waiting before rtsp reconnect"
        );
        sleep_with_shutdown(settings.rtsp_retry_delay_seconds, shutdown);
    }
}

fn stream_once(
    source: StreamSource,
    input: &InputSource,
    settings: &MotionDetectionConfig,
    dimensions: FrameDimensions,
    sender: &mpsc::Sender<StreamMessage>,
    shutdown: &Arc<AtomicBool>,
) -> Result<(), AppError> {
    let (frame_sender, frame_receiver) = mpsc::sync_channel::<VideoFrame>(2);
    let input_clone = input.clone();
    let settings_clone = settings.clone();
    let shutdown_clone = shutdown.clone();

    let reader = thread::spawn(move || {
        stream_input(
            &input_clone,
            &settings_clone,
            dimensions,
            &frame_sender,
            &shutdown_clone,
        )
    });

    for frame in frame_receiver {
        if sender.send(StreamMessage::Frame { source, frame }).is_err() {
            shutdown.store(true, Ordering::Relaxed);
            break;
        }
    }

    match reader.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) if shutdown.load(Ordering::Relaxed) => {
            if matches!(error, crate::ffmpeg::StreamError::FrameChannelClosed) {
                Ok(())
            } else {
                Err(AppError::from(error))
            }
        }
        Ok(Err(error)) => {
            shutdown.store(true, Ordering::Relaxed);
            Err(AppError::from(error))
        }
        Err(_) => {
            shutdown.store(true, Ordering::Relaxed);
            Err(match source {
                StreamSource::Detection => AppError::DetectionStreamThread,
                StreamSource::Output => AppError::OutputStreamThread,
            })
        }
    }
}

fn spawn_processor(
    input: InputSource,
    settings: MotionDetectionConfig,
    stream_receiver: mpsc::Receiver<StreamMessage>,
    file_writer: Option<FileOutputWriter>,
    mqtt_sender: Option<mpsc::Sender<Vec<u8>>>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("motion-processor".to_owned())
        .spawn(move || {
            let mut detector = MotionDetector::new(&settings);
            let mut session_tracker = MotionSessionTracker::new(&settings);
            let mut detection_index_state = FrameIndexState::default();
            let mut output_index_state = FrameIndexState::default();
            let mut processor_state =
                ProcessorState::new(input, settings, file_writer, mqtt_sender);

            for message in stream_receiver {
                match message {
                    StreamMessage::Frame {
                        source: StreamSource::Detection,
                        frame,
                    } => {
                        let frame = detection_index_state.normalize(frame);
                        let analysis = detector.analyze(&frame);
                        let events = session_tracker.ingest_events(frame, analysis);
                        processor_state.handle_session_events(events);
                    }
                    StreamMessage::Frame {
                        source: StreamSource::Output,
                        frame,
                    } => {
                        let frame = output_index_state.normalize(frame);
                        processor_state.push_output_frame(frame);
                    }
                }
            }

            let final_events = session_tracker.finish_events();
            processor_state.handle_session_events(final_events);
            processor_state.resolve_pending_requests(true);
            processor_state.try_emit_finished_sessions();
        })
        .unwrap_or_else(|error| panic!("failed to spawn processor thread: {error}"))
}

struct ProcessorState {
    input: InputSource,
    settings: MotionDetectionConfig,
    file_writer: Option<FileOutputWriter>,
    mqtt_sender: Option<mpsc::Sender<Vec<u8>>>,
    output_buffer: OutputFrameBuffer,
    pending_requests: Vec<MotionSnapshotSelection>,
    resolved_snapshots: HashMap<u64, Vec<ResolvedSnapshot>>,
    finished_sessions: HashMap<u64, MotionSessionSummary>,
}

impl ProcessorState {
    fn new(
        input: InputSource,
        settings: MotionDetectionConfig,
        file_writer: Option<FileOutputWriter>,
        mqtt_sender: Option<mpsc::Sender<Vec<u8>>>,
    ) -> Self {
        Self {
            output_buffer: OutputFrameBuffer::new(&settings),
            input,
            settings,
            file_writer,
            mqtt_sender,
            pending_requests: Vec::new(),
            resolved_snapshots: HashMap::new(),
            finished_sessions: HashMap::new(),
        }
    }

    fn handle_session_events(&mut self, events: Vec<MotionSessionEvent>) {
        for event in events {
            match event {
                MotionSessionEvent::SnapshotSelected(selection) => {
                    if let Some(frame) = self
                        .output_buffer
                        .resolve(selection.frame.captured_at, false)
                    {
                        self.resolved_snapshots
                            .entry(selection.session_id)
                            .or_default()
                            .push(ResolvedSnapshot {
                                frame,
                                event: selection.event,
                            });
                    } else {
                        self.pending_requests.push(selection);
                    }
                }
                MotionSessionEvent::SessionFinished(summary) => {
                    self.finished_sessions.insert(summary.session_id, summary);
                }
            }
        }

        self.try_emit_finished_sessions();
    }

    fn push_output_frame(&mut self, frame: VideoFrame) {
        self.output_buffer.push(frame);
        self.resolve_pending_requests(false);
        self.try_emit_finished_sessions();
    }

    fn resolve_pending_requests(&mut self, force: bool) {
        let mut still_pending = Vec::new();

        for request in self.pending_requests.drain(..) {
            match self.output_buffer.resolve(request.frame.captured_at, force) {
                Some(frame) => {
                    self.resolved_snapshots
                        .entry(request.session_id)
                        .or_default()
                        .push(ResolvedSnapshot {
                            frame,
                            event: request.event,
                        });
                }
                None => still_pending.push(request),
            }
        }

        self.pending_requests = still_pending;
    }

    fn try_emit_finished_sessions(&mut self) {
        let session_ids: Vec<u64> = self.finished_sessions.keys().copied().collect();

        for session_id in session_ids {
            if self
                .pending_requests
                .iter()
                .any(|request| request.session_id == session_id)
            {
                continue;
            }

            let Some(summary) = self.finished_sessions.remove(&session_id) else {
                continue;
            };
            let Some(mut snapshots) = self.resolved_snapshots.remove(&session_id) else {
                warn!(
                    session_id,
                    "motion session finished without a resolved snapshot"
                );
                continue;
            };

            snapshots.sort_by_key(|snapshot| snapshot.frame.index);
            snapshots.dedup_by_key(|snapshot| snapshot.frame.index);

            let captures: Vec<MotionSessionCapture> = snapshots
                .into_iter()
                .map(|snapshot| MotionSessionCapture {
                    frame: snapshot.frame,
                    event: snapshot.event,
                    motion_started_at: summary.motion_started_at,
                    motion_started_frame_index: summary.motion_started_frame_index,
                    motion_ended_at: summary.motion_ended_at,
                    motion_ended_frame_index: summary.motion_ended_frame_index,
                })
                .collect();

            let _ = process_completed_captures(
                &self.input,
                &self.settings,
                captures,
                self.file_writer.as_ref(),
                self.mqtt_sender.as_ref(),
            );
        }
    }
}

fn process_completed_captures(
    input: &InputSource,
    settings: &MotionDetectionConfig,
    captures: Vec<MotionSessionCapture>,
    file_writer: Option<&FileOutputWriter>,
    mqtt_sender: Option<&mpsc::Sender<Vec<u8>>>,
) -> bool {
    for completed_capture in captures {
        match MotionCapture::from_session_capture(
            input,
            &completed_capture,
            settings.snapshot_jpeg_quality,
        ) {
            Ok(capture) => {
                if let Some(writer) = file_writer {
                    match writer.write_capture(&capture) {
                        Ok(paths) => info!(
                            image_path = %paths.image_path.display(),
                            metadata_path = %paths.metadata_path.display(),
                            "wrote motion capture to files"
                        ),
                        Err(error) => error!(?error, "failed to write motion capture files"),
                    }
                }

                if let Some(sender) = mqtt_sender {
                    match capture.mqtt_payload() {
                        Ok(payload) => {
                            if let Err(error) = sender.send(payload) {
                                error!(?error, "failed to enqueue mqtt payload");
                                return false;
                            }
                        }
                        Err(error) => error!(?error, "failed to serialize mqtt payload"),
                    }
                }

                info!(
                    frame_index = capture.frame_index,
                    motion_started_frame_index = capture.motion_started_frame_index,
                    motion_ended_frame_index = capture.motion_ended_frame_index,
                    motion_ratio = capture.motion_ratio,
                    local_motion_ratio = capture.local_motion_ratio,
                    motion_duration_ms = capture.motion_duration_ms,
                    "motion capture completed"
                );
            }
            Err(error) => error!(?error, "failed to build motion capture"),
        }
    }

    true
}

fn sleep_with_shutdown(seconds: u64, shutdown: &Arc<AtomicBool>) {
    for _ in 0..seconds {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }
}

#[derive(Clone, Copy, Debug)]
enum StreamSource {
    Detection,
    Output,
}

enum StreamMessage {
    Frame {
        source: StreamSource,
        frame: VideoFrame,
    },
}

#[derive(Default)]
struct FrameIndexState {
    offset: u64,
    last_raw_index: Option<u64>,
}

impl FrameIndexState {
    fn normalize(&mut self, mut frame: VideoFrame) -> VideoFrame {
        if let Some(last_raw_index) = self.last_raw_index
            && frame.index < last_raw_index
        {
            self.offset = self.offset.saturating_add(last_raw_index.saturating_add(1));
        }

        self.last_raw_index = Some(frame.index);
        frame.index = frame.index.saturating_add(self.offset);
        frame
    }
}

struct OutputFrameBuffer {
    frames: VecDeque<VideoFrame>,
    max_age: Duration,
}

impl OutputFrameBuffer {
    fn new(settings: &MotionDetectionConfig) -> Self {
        let buffer_seconds = settings
            .motion_snapshot_delay_seconds
            .saturating_add(settings.motion_end_grace_seconds)
            .saturating_add(2);

        Self {
            frames: VecDeque::new(),
            max_age: Duration::from_secs(buffer_seconds),
        }
    }

    fn push(&mut self, frame: VideoFrame) {
        self.frames.push_back(frame);
        self.prune();
    }

    fn resolve(&self, target_time: SystemTime, force: bool) -> Option<VideoFrame> {
        let latest_time = self.frames.back().map(|frame| frame.captured_at)?;
        if !force && latest_time.duration_since(target_time).is_err() {
            return None;
        }

        self.frames
            .iter()
            .min_by_key(|frame| abs_time_diff(frame.captured_at, target_time))
            .cloned()
    }

    fn prune(&mut self) {
        let Some(latest_time) = self.frames.back().map(|frame| frame.captured_at) else {
            return;
        };

        while let Some(oldest) = self.frames.front() {
            match latest_time.duration_since(oldest.captured_at) {
                Ok(age) if age > self.max_age => {
                    self.frames.pop_front();
                }
                _ => break,
            }
        }
    }
}

#[derive(Clone)]
struct ResolvedSnapshot {
    frame: VideoFrame,
    event: MotionEvent,
}

fn abs_time_diff(left: SystemTime, right: SystemTime) -> Duration {
    match left.duration_since(right) {
        Ok(duration) => duration,
        Err(error) => error.duration(),
    }
}
