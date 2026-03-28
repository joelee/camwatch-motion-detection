use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, atomic::AtomicBool, mpsc},
    thread,
};

use camwatch_motion_detection::{
    config::{InputSource, MotionDetectionConfig},
    ffmpeg::{StreamOptions, VideoFrame, stream_input_with_options},
    motion::MotionDetector,
};

const FIXTURE_DIRECTORY: &str = "tests/video";
const DETECTION_TOLERANCE_SECONDS: u64 = 1;

#[test]
fn video_fixtures_match_expected_motion_start() {
    if !ffmpeg_is_available() {
        eprintln!("skipping video fixture tests because ffmpeg is not available on PATH");
        return;
    }

    let fixtures = match discover_fixtures(Path::new(FIXTURE_DIRECTORY)) {
        Ok(fixtures) => fixtures,
        Err(error) => panic!("failed to discover video fixtures: {error}"),
    };

    if fixtures.is_empty() {
        eprintln!(
            "skipping video fixture tests because {} has no .mp4 files",
            FIXTURE_DIRECTORY
        );
        return;
    }

    let mut failures = Vec::new();

    for fixture in fixtures {
        if let Err(message) = assert_fixture(&fixture) {
            failures.push(message);
        }
    }

    if !failures.is_empty() {
        panic!("{}", failures.join("\n\n"));
    }
}

fn assert_fixture(fixture: &VideoFixture) -> Result<(), String> {
    let settings = fixture_test_config();
    let first_detection_frame = run_motion_detection(fixture, &settings)?;
    let detected_second = first_detection_frame.map(|frame| frame_to_seconds(frame, &settings));

    match (fixture.expectation, first_detection_frame) {
        (FixtureExpectation::NoMotion, None) => {
            eprintln!(
                "fixture {}: expected no motion, detected not at all",
                fixture.path.display()
            );
            Ok(())
        }
        (FixtureExpectation::NoMotion, Some(frame)) => Err(format!(
            "fixture {}: expected no motion, detected at frame {} ({})",
            fixture.path.display(),
            frame,
            format_detected_second(detected_second),
        )),
        (FixtureExpectation::MotionStartsAt(expected_second), None) => Err(format!(
            "fixture {}: expected motion starting near second {}, detected not at all",
            fixture.path.display(),
            expected_second,
        )),
        (FixtureExpectation::MotionStartsAt(expected_second), Some(frame)) => {
            let expected_start_frame = expected_second * u64::from(settings.frame_rate);
            let latest_allowed_frame =
                (expected_second + DETECTION_TOLERANCE_SECONDS) * u64::from(settings.frame_rate);

            if frame < expected_start_frame {
                return Err(format!(
                    "fixture {}: detected too early at frame {} ({}), expected at or after second {}",
                    fixture.path.display(),
                    frame,
                    format_detected_second(detected_second),
                    expected_second,
                ));
            }

            if frame > latest_allowed_frame {
                return Err(format!(
                    "fixture {}: detected too late at frame {} ({}), expected by second {} (+{}s tolerance)",
                    fixture.path.display(),
                    frame,
                    format_detected_second(detected_second),
                    expected_second,
                    DETECTION_TOLERANCE_SECONDS,
                ));
            }

            eprintln!(
                "fixture {}: expected motion near second {}, detected at {}",
                fixture.path.display(),
                expected_second,
                format_detected_second(detected_second),
            );

            Ok(())
        }
    }
}

fn frame_to_seconds(frame: u64, settings: &MotionDetectionConfig) -> f64 {
    frame as f64 / f64::from(settings.frame_rate)
}

fn format_detected_second(detected_second: Option<f64>) -> String {
    match detected_second {
        Some(second) => format!("{second:.2}s"),
        None => "not at all".to_owned(),
    }
}

fn run_motion_detection(
    fixture: &VideoFixture,
    settings: &MotionDetectionConfig,
) -> Result<Option<u64>, String> {
    let input = InputSource::File(fixture.path.clone());
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
                &frame_sender,
                &stream_shutdown,
                StreamOptions {
                    realtime_for_files: false,
                },
            )
        }
    });

    let mut detector = MotionDetector::new(settings);
    let mut first_detection_frame = None;

    for frame in frame_receiver {
        if first_detection_frame.is_none() && detector.analyze(&frame).is_some() {
            first_detection_frame = Some(frame.index);
        }
    }

    match stream_handle.join() {
        Ok(Ok(())) => Ok(first_detection_frame),
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

fn fixture_test_config() -> MotionDetectionConfig {
    MotionDetectionConfig {
        frame_width: 320,
        frame_height: 180,
        frame_rate: 5,
        pixel_difference_threshold: 20,
        motion_ratio_threshold: 0.015,
        background_alpha: 0.08,
        event_cooldown_seconds: 0,
        ..MotionDetectionConfig::default()
    }
}

fn discover_fixtures(directory: &Path) -> Result<Vec<VideoFixture>, String> {
    if !directory.exists() {
        return Ok(Vec::new());
    }

    let mut fixtures = Vec::new();
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("failed to read {}: {error}", directory.display()))?;

    for entry in entries {
        let entry = entry.map_err(|error| format!("failed to read fixture entry: {error}"))?;
        let path = entry.path();

        if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("mp4") {
            continue;
        }

        fixtures.push(VideoFixture::from_path(path)?);
    }

    fixtures.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(fixtures)
}

fn ffmpeg_is_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[derive(Clone, Debug)]
struct VideoFixture {
    path: PathBuf,
    expectation: FixtureExpectation,
}

impl VideoFixture {
    fn from_path(path: PathBuf) -> Result<Self, String> {
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| {
                format!(
                    "fixture path {} does not have a valid UTF-8 filename",
                    path.display()
                )
            })?;

        let suffix = stem.rsplit('-').next().ok_or_else(|| {
            format!(
                "fixture {} must end with '-<seconds>' or '-none' before .mp4",
                path.display()
            )
        })?;

        let expectation = if suffix.eq_ignore_ascii_case("none") {
            FixtureExpectation::NoMotion
        } else {
            let second = suffix.parse::<u64>().map_err(|_| {
                format!(
                    "fixture {} must end with '-<seconds>' or '-none' before .mp4",
                    path.display()
                )
            })?;
            FixtureExpectation::MotionStartsAt(second)
        };

        Ok(Self { path, expectation })
    }
}

#[derive(Clone, Copy, Debug)]
enum FixtureExpectation {
    MotionStartsAt(u64),
    NoMotion,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{FixtureExpectation, VideoFixture};

    #[test]
    fn parses_motion_suffix_from_filename() {
        let fixture = match VideoFixture::from_path(PathBuf::from("tests/video/test_video-12.mp4"))
        {
            Ok(fixture) => fixture,
            Err(error) => panic!("expected fixture name to parse, got {error}"),
        };

        assert!(matches!(
            fixture.expectation,
            FixtureExpectation::MotionStartsAt(12)
        ));
    }

    #[test]
    fn parses_none_suffix_from_filename() {
        let fixture =
            match VideoFixture::from_path(PathBuf::from("tests/video/quiet_room-none.mp4")) {
                Ok(fixture) => fixture,
                Err(error) => panic!("expected fixture name to parse, got {error}"),
            };

        assert!(matches!(fixture.expectation, FixtureExpectation::NoMotion));
    }

    #[test]
    fn rejects_invalid_fixture_name() {
        let result = VideoFixture::from_path(PathBuf::from("tests/video/quiet_room.mp4"));

        assert!(result.is_err());
    }
}
