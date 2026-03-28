//! Frame ingestion through the system `ffmpeg` binary.
//!
//! We deliberately shell out to `ffmpeg` instead of linking a large video stack into the Rust
//! binary. `ffmpeg` handles codecs, RTSP, scaling, and frame-rate limiting for us.

use std::{
    io::{self, BufRead, BufReader, Read},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
    },
    thread,
    time::SystemTime,
};

use thiserror::Error;
use tracing::{debug, warn};

use crate::config::{InputSource, MotionDetectionConfig};

#[derive(Clone, Copy, Debug)]
pub struct StreamOptions {
    pub realtime_for_files: bool,
}

impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            realtime_for_files: true,
        }
    }
}

#[derive(Debug)]
pub struct VideoFrame {
    pub index: u64,
    pub captured_at: SystemTime,
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

pub fn stream_input(
    input: &InputSource,
    settings: &MotionDetectionConfig,
    frame_sender: &SyncSender<VideoFrame>,
    shutdown: &Arc<AtomicBool>,
) -> Result<(), StreamError> {
    stream_input_with_options(
        input,
        settings,
        frame_sender,
        shutdown,
        StreamOptions::default(),
    )
}

pub fn stream_input_with_options(
    input: &InputSource,
    settings: &MotionDetectionConfig,
    frame_sender: &SyncSender<VideoFrame>,
    shutdown: &Arc<AtomicBool>,
    options: StreamOptions,
) -> Result<(), StreamError> {
    let frame_size = frame_size_bytes(settings.frame_width, settings.frame_height)?;
    let mut command = Command::new("ffmpeg");
    command
        .args(build_ffmpeg_args_with_options(input, settings, options))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().map_err(StreamError::SpawnFfmpeg)?;
    let stdout = child.stdout.take().ok_or(StreamError::MissingStdout)?;
    let stderr = child.stderr.take().ok_or(StreamError::MissingStderr)?;

    // `ffmpeg` writes diagnostics to stderr. Reading it on a side thread avoids deadlocks if the
    // pipe fills up while the main thread is busy consuming raw video frames from stdout.
    let stderr_handle = thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(message) if !message.trim().is_empty() => warn!(target = "ffmpeg", "{message}"),
                Ok(_) => {}
                Err(error) => {
                    warn!(target = "ffmpeg", ?error, "failed to read ffmpeg stderr");
                    break;
                }
            }
        }
    });

    let mut output = BufReader::new(stdout);
    let mut index = 0_u64;

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Each frame is a flat RGB byte buffer: width * height * 3 bytes.
        let mut rgb = vec![0_u8; frame_size];
        match output.read_exact(&mut rgb) {
            Ok(()) => {
                frame_sender
                    .send(VideoFrame {
                        index,
                        captured_at: SystemTime::now(),
                        width: settings.frame_width,
                        height: settings.frame_height,
                        rgb,
                    })
                    .map_err(|_| StreamError::FrameChannelClosed)?;
                index = index.saturating_add(1);
            }
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                debug!(frames_read = index, "ffmpeg stream reached EOF");
                break;
            }
            Err(error) => {
                let _ = child.kill();
                let _ = stderr_handle.join();
                return Err(StreamError::ReadFrame(error));
            }
        }
    }

    if shutdown.load(Ordering::Relaxed) {
        let _ = child.kill();
    }

    let status = child.wait().map_err(StreamError::WaitForFfmpeg)?;
    let _ = stderr_handle.join();

    if !status.success() && !shutdown.load(Ordering::Relaxed) {
        return Err(StreamError::FfmpegExited(status.code()));
    }

    Ok(())
}

fn frame_size_bytes(width: u32, height: u32) -> Result<usize, StreamError> {
    let pixels = u64::from(width) * u64::from(height) * 3;
    usize::try_from(pixels).map_err(|_| StreamError::FrameTooLarge)
}

pub fn build_ffmpeg_args(input: &InputSource, settings: &MotionDetectionConfig) -> Vec<String> {
    build_ffmpeg_args_with_options(input, settings, StreamOptions::default())
}

fn build_ffmpeg_args_with_options(
    input: &InputSource,
    settings: &MotionDetectionConfig,
    options: StreamOptions,
) -> Vec<String> {
    let mut args = vec![
        "-hide_banner".to_owned(),
        "-loglevel".to_owned(),
        "error".to_owned(),
        "-nostdin".to_owned(),
    ];

    if options.realtime_for_files && matches!(input, InputSource::File(_)) {
        args.push("-re".to_owned());
    }

    if let InputSource::Rtsp(_) = input {
        args.extend([
            "-rtsp_transport".to_owned(),
            settings.rtsp_transport.as_ffmpeg_value().to_owned(),
            "-fflags".to_owned(),
            "nobuffer".to_owned(),
            "-flags".to_owned(),
            "low_delay".to_owned(),
        ]);
    }

    args.extend(["-i".to_owned(), input.display_value()]);

    // `scale + pad` preserves aspect ratio while still producing a fixed-size frame buffer for the
    // motion detector. That keeps CPU usage predictable and simplifies downstream code.
    let filter = format!(
        "fps={},scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:(ow-iw)/2:(oh-ih)/2:color=black",
        settings.frame_rate,
        settings.frame_width,
        settings.frame_height,
        settings.frame_width,
        settings.frame_height,
    );

    args.extend([
        "-an".to_owned(),
        "-vf".to_owned(),
        filter,
        "-pix_fmt".to_owned(),
        "rgb24".to_owned(),
        "-f".to_owned(),
        "rawvideo".to_owned(),
        "pipe:1".to_owned(),
    ]);

    args
}

#[derive(Debug, Error)]
pub enum StreamError {
    #[error("failed to spawn ffmpeg: {0}")]
    SpawnFfmpeg(io::Error),
    #[error("ffmpeg stdout was not piped")]
    MissingStdout,
    #[error("ffmpeg stderr was not piped")]
    MissingStderr,
    #[error("failed to read frame from ffmpeg: {0}")]
    ReadFrame(io::Error),
    #[error("failed while waiting for ffmpeg to exit: {0}")]
    WaitForFfmpeg(io::Error),
    #[error("ffmpeg exited with code {0:?}")]
    FfmpegExited(Option<i32>),
    #[error("frame size exceeds supported memory limits")]
    FrameTooLarge,
    #[error("frame channel closed before stream completed")]
    FrameChannelClosed,
}

#[cfg(test)]
mod tests {
    use super::{StreamOptions, build_ffmpeg_args, build_ffmpeg_args_with_options};
    use crate::config::{InputSource, MotionDetectionConfig, RtspTransport};

    #[test]
    fn file_input_uses_realtime_flag() {
        let args = build_ffmpeg_args(
            &InputSource::File("movie.mp4".into()),
            &MotionDetectionConfig::default(),
        );

        assert!(args.iter().any(|item| item == "-re"));
        assert!(args.iter().any(|item| item == "movie.mp4"));
    }

    #[test]
    fn rtsp_input_uses_transport_settings() {
        let settings = MotionDetectionConfig {
            rtsp_transport: RtspTransport::Udp,
            ..MotionDetectionConfig::default()
        };
        let args = build_ffmpeg_args(
            &InputSource::Rtsp("rtsp://camera/live".to_owned()),
            &settings,
        );

        assert!(args.iter().any(|item| item == "-rtsp_transport"));
        assert!(args.iter().any(|item| item == "udp"));
        assert!(!args.iter().any(|item| item == "-re"));
    }

    #[test]
    fn test_options_can_disable_realtime_file_reading() {
        let args = build_ffmpeg_args_with_options(
            &InputSource::File("movie.mp4".into()),
            &MotionDetectionConfig::default(),
            StreamOptions {
                realtime_for_files: false,
            },
        );

        assert!(!args.iter().any(|item| item == "-re"));
    }
}
