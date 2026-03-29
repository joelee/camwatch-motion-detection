mod common;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool, mpsc},
    thread,
};

use camwatch_motion_detection::{
    config::InputSource,
    ffmpeg::{StreamOptions, VideoFrame, resolve_output_dimensions, stream_input_with_options},
    motion::MotionDetector,
    output::{FileOutputWriter, MotionCapture},
    session::MotionSessionTracker,
};
use common::{
    FIXTURE_DIRECTORY, FixtureExpectation, OUTPUT_DIRECTORY, VideoFixture, discover_fixtures,
    ffmpeg_is_available, fixture_test_config,
};

#[test]
fn file_output_writes_expected_artifacts_for_video_fixtures() {
    if !ffmpeg_is_available() {
        eprintln!("skipping file output tests because ffmpeg is not available on PATH");
        return;
    }

    let fixtures = match discover_fixtures(Path::new(FIXTURE_DIRECTORY)) {
        Ok(fixtures) => fixtures,
        Err(error) => panic!("failed to discover video fixtures: {error}"),
    };

    if fixtures.is_empty() {
        eprintln!(
            "skipping file output tests because {} has no .mp4 files",
            FIXTURE_DIRECTORY
        );
        return;
    }

    let output_root = Path::new(OUTPUT_DIRECTORY);
    if output_root.exists() {
        fs::remove_dir_all(output_root)
            .unwrap_or_else(|error| panic!("failed to clear {}: {error}", output_root.display()));
    }
    fs::create_dir_all(output_root)
        .unwrap_or_else(|error| panic!("failed to create {}: {error}", output_root.display()));

    let mut failures = Vec::new();

    for fixture in fixtures {
        if let Err(message) = assert_fixture_output(&fixture, output_root) {
            failures.push(message);
        }
    }

    if !failures.is_empty() {
        panic!("{}", failures.join("\n\n"));
    }
}

fn assert_fixture_output(fixture: &VideoFixture, output_root: &Path) -> Result<(), String> {
    let fixture_name = fixture.stem()?;
    let fixture_output_dir = output_root.join(&fixture_name);

    if fixture_output_dir.exists() {
        fs::remove_dir_all(&fixture_output_dir).map_err(|error| {
            format!(
                "failed to clear fixture output directory {}: {error}",
                fixture_output_dir.display()
            )
        })?;
    }

    let captures = run_captures(fixture)?;

    match (fixture.expectation, captures.as_slice()) {
        (FixtureExpectation::NoMotion, []) => {
            eprintln!(
                "fixture {}: no motion capture written, as expected",
                fixture.path.display()
            );
            Ok(())
        }
        (FixtureExpectation::NoMotion, [capture, ..]) => {
            let writer = FileOutputWriter::new(fixture_output_dir.clone()).map_err(|error| {
                format!("failed to create {}: {error}", fixture_output_dir.display())
            })?;
            let paths = writer.write_capture(capture).map_err(|error| {
                format!(
                    "failed to write unexpected capture for {}: {error}",
                    fixture.path.display()
                )
            })?;

            Err(format!(
                "fixture {} should not emit files, but wrote {} and {}",
                fixture.path.display(),
                paths.image_path.display(),
                paths.metadata_path.display(),
            ))
        }
        (FixtureExpectation::MotionStartsAt(expected_second), []) => Err(format!(
            "fixture {} should write output near second {}, but no motion capture was produced",
            fixture.path.display(),
            expected_second,
        )),
        (FixtureExpectation::MotionStartsAt(expected_second), captures) => {
            let writer = FileOutputWriter::new(fixture_output_dir.clone()).map_err(|error| {
                format!("failed to create {}: {error}", fixture_output_dir.display())
            })?;
            let mut image_files = Vec::new();
            let mut metadata_files = Vec::new();

            for capture in captures {
                let paths = writer.write_capture(capture).map_err(|error| {
                    format!(
                        "failed to write capture for {}: {error}",
                        fixture.path.display()
                    )
                })?;
                let metadata = fs::read_to_string(&paths.metadata_path).map_err(|error| {
                    format!(
                        "failed to read metadata file {}: {error}",
                        paths.metadata_path.display()
                    )
                })?;
                let image_name = paths
                    .image_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| {
                        format!("invalid output filename {}", paths.image_path.display())
                    })?;
                let metadata_name = paths
                    .metadata_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| {
                        format!("invalid output filename {}", paths.metadata_path.display())
                    })?;

                if !paths.image_path.is_file() || !paths.metadata_path.is_file() {
                    return Err(format!(
                        "fixture {} did not create both output files under {}",
                        fixture.path.display(),
                        fixture_output_dir.display(),
                    ));
                }

                if !image_name.starts_with("motion-") || !image_name.ends_with(".jpg") {
                    return Err(format!(
                        "fixture {} wrote unexpected image filename {}",
                        fixture.path.display(),
                        image_name,
                    ));
                }

                if !metadata_name.starts_with("motion-") || !metadata_name.ends_with(".toml") {
                    return Err(format!(
                        "fixture {} wrote unexpected metadata filename {}",
                        fixture.path.display(),
                        metadata_name,
                    ));
                }

                if !metadata.contains(&format!("snapshot_file = \"{image_name}\"")) {
                    return Err(format!(
                        "fixture {} metadata does not reference {}",
                        fixture.path.display(),
                        image_name,
                    ));
                }
                if !metadata.contains("motion_started_at_epoch_ms")
                    || !metadata.contains("motion_ended_at_epoch_ms")
                    || !metadata.contains("motion_duration_ms")
                {
                    return Err(format!(
                        "fixture {} metadata is missing session timing fields",
                        fixture.path.display(),
                    ));
                }

                image_files.push(paths.image_path);
                metadata_files.push(paths.metadata_path);
            }

            eprintln!(
                "fixture {}: wrote {} image(s) and {} metadata file(s) for expected motion near second {}",
                fixture.path.display(),
                image_files.len(),
                metadata_files.len(),
                expected_second,
            );
            Ok(())
        }
    }
}

fn run_captures(fixture: &VideoFixture) -> Result<Vec<MotionCapture>, String> {
    let mut settings = fixture_test_config();
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
    let mut captures = Vec::new();

    for frame in frame_receiver {
        let analysis = detector.analyze(&frame);
        for completed_capture in session_tracker.ingest(frame, analysis) {
            captures.push(
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

    for completed_capture in session_tracker.finish() {
        captures.push(
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
        Ok(Ok(())) => Ok(captures),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_tests_output_root() {
        assert_eq!(
            PathBuf::from(OUTPUT_DIRECTORY),
            PathBuf::from("tests/output")
        );
    }
}
