mod common;

use std::{
    fs,
    path::Path,
    sync::{Arc, atomic::AtomicBool, mpsc},
    thread,
};

use camwatch_motion_detection::{
    config::{InputSource, MotionDetectionConfig},
    ffmpeg::{StreamOptions, VideoFrame, resolve_output_dimensions, stream_input_with_options},
    motion::MotionDetector,
    output::{FileOutputWriter, MotionCapture},
    session::MotionSessionTracker,
};
use common::{
    FIXTURE_DIRECTORY, OUTPUT_DIRECTORY, VideoFixture, discover_fixtures, ffmpeg_is_available,
    fixture_test_config,
};
use image::GenericImageView;

#[test]
fn output_image_matches_source_resolution_when_output_dimensions_are_omitted() {
    if !ffmpeg_is_available() {
        eprintln!("skipping output dimension test because ffmpeg is not available on PATH");
        return;
    }

    let fixture =
        motion_fixture().unwrap_or_else(|error| panic!("failed to find motion fixture: {error}"));
    let capture = first_capture_for_fixture(&fixture, fixture_test_config())
        .unwrap_or_else(|error| panic!("failed to build capture: {error}"));

    let output_root = Path::new(OUTPUT_DIRECTORY).join("dimension-source");
    if output_root.exists() {
        fs::remove_dir_all(&output_root)
            .unwrap_or_else(|error| panic!("failed to clear {}: {error}", output_root.display()));
    }

    let writer = FileOutputWriter::new(output_root.clone())
        .unwrap_or_else(|error| panic!("failed to create {}: {error}", output_root.display()));
    let paths = writer
        .write_capture(&capture)
        .unwrap_or_else(|error| panic!("failed to write capture: {error}"));

    let dimensions = image::ImageReader::open(&paths.image_path)
        .unwrap_or_else(|error| panic!("failed to open {}: {error}", paths.image_path.display()))
        .decode()
        .unwrap_or_else(|error| panic!("failed to decode {}: {error}", paths.image_path.display()))
        .dimensions();

    assert_eq!(dimensions, (1920, 1080));
}

#[test]
fn output_image_matches_configured_output_resolution() {
    if !ffmpeg_is_available() {
        eprintln!("skipping output dimension test because ffmpeg is not available on PATH");
        return;
    }

    let fixture =
        motion_fixture().unwrap_or_else(|error| panic!("failed to find motion fixture: {error}"));
    let mut settings = fixture_test_config();
    settings.output_frame_width = Some(1280);
    settings.output_frame_height = Some(720);

    let capture = first_capture_for_fixture(&fixture, settings)
        .unwrap_or_else(|error| panic!("failed to build capture: {error}"));

    let output_root = Path::new(OUTPUT_DIRECTORY).join("dimension-custom");
    if output_root.exists() {
        fs::remove_dir_all(&output_root)
            .unwrap_or_else(|error| panic!("failed to clear {}: {error}", output_root.display()));
    }

    let writer = FileOutputWriter::new(output_root.clone())
        .unwrap_or_else(|error| panic!("failed to create {}: {error}", output_root.display()));
    let paths = writer
        .write_capture(&capture)
        .unwrap_or_else(|error| panic!("failed to write capture: {error}"));

    let dimensions = image::ImageReader::open(&paths.image_path)
        .unwrap_or_else(|error| panic!("failed to open {}: {error}", paths.image_path.display()))
        .decode()
        .unwrap_or_else(|error| panic!("failed to decode {}: {error}", paths.image_path.display()))
        .dimensions();

    assert_eq!(dimensions, (1280, 720));
}

fn motion_fixture() -> Result<VideoFixture, String> {
    let fixtures = discover_fixtures(Path::new(FIXTURE_DIRECTORY))?;
    fixtures
        .into_iter()
        .find(|fixture| {
            matches!(
                fixture.expectation,
                common::FixtureExpectation::MotionStartsAt(_)
            )
        })
        .ok_or_else(|| format!("no motion fixture found in {}", FIXTURE_DIRECTORY))
}

fn first_capture_for_fixture(
    fixture: &VideoFixture,
    mut settings: MotionDetectionConfig,
) -> Result<MotionCapture, String> {
    settings.event_cooldown_seconds = 3600;
    let input = InputSource::File(fixture.path.clone());
    let output_dimensions = resolve_output_dimensions(&input, &settings).map_err(|error| {
        format!(
            "failed to resolve output dimensions for {}: {error}",
            fixture.path.display()
        )
    })?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let (frame_sender, frame_receiver) = mpsc::sync_channel::<VideoFrame>(4);
    let stream_shutdown = Arc::clone(&shutdown);

    let stream_handle = thread::spawn({
        let input = input.clone();
        let settings = settings.clone();
        move || {
            stream_input_with_options(
                &input,
                &settings,
                output_dimensions,
                &frame_sender,
                &stream_shutdown,
                StreamOptions {
                    realtime_for_files: false,
                },
            )
        }
    });

    let mut detector = MotionDetector::new(&settings);
    let mut session_tracker = MotionSessionTracker::new(&settings);
    let mut first_capture = None;

    for frame in frame_receiver {
        let analysis = detector.analyze(&frame);
        for completed_capture in session_tracker.ingest(frame, analysis) {
            if first_capture.is_none() {
                first_capture = Some(
                    MotionCapture::from_session_capture(
                        &input,
                        &completed_capture,
                        settings.snapshot_jpeg_quality,
                    )
                    .map_err(|error| {
                        format!(
                            "failed to build motion capture for {}: {error}",
                            fixture.path.display()
                        )
                    })?,
                );
            }
        }
    }

    if first_capture.is_none()
        && let Some(completed_capture) = session_tracker.finish().into_iter().next()
    {
        first_capture = Some(
            MotionCapture::from_session_capture(
                &input,
                &completed_capture,
                settings.snapshot_jpeg_quality,
            )
            .map_err(|error| {
                format!(
                    "failed to build motion capture for {}: {error}",
                    fixture.path.display()
                )
            })?,
        );
    }

    match stream_handle.join() {
        Ok(Ok(())) => first_capture.ok_or_else(|| {
            format!(
                "expected {} to produce at least one motion capture",
                fixture.path.display()
            )
        }),
        Ok(Err(error)) => Err(format!(
            "{} failed during ffmpeg processing: {error}",
            fixture.path.display()
        )),
        Err(_) => Err(format!(
            "{} caused the streaming thread to panic",
            fixture.path.display()
        )),
    }
}
