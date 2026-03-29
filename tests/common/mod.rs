#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use camwatch_motion_detection::config::MotionDetectionConfig;

pub const FIXTURE_DIRECTORY: &str = "tests/video";
pub const OUTPUT_DIRECTORY: &str = "tests/output";

#[derive(Clone, Debug)]
pub struct VideoFixture {
    pub path: PathBuf,
    pub expectation: FixtureExpectation,
}

impl VideoFixture {
    pub fn from_path(path: PathBuf) -> Result<Self, String> {
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

    pub fn stem(&self) -> Result<String, String> {
        self.path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_owned)
            .ok_or_else(|| {
                format!(
                    "fixture path {} does not have a valid UTF-8 filename",
                    self.path.display()
                )
            })
    }
}

#[derive(Clone, Copy, Debug)]
pub enum FixtureExpectation {
    MotionStartsAt(u64),
    NoMotion,
}

pub fn discover_fixtures(directory: &Path) -> Result<Vec<VideoFixture>, String> {
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

pub fn ffmpeg_is_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|output| output.status.success())
}

pub fn fixture_test_config() -> MotionDetectionConfig {
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
