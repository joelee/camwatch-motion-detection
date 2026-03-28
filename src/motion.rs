//! Lightweight motion scoring.
//!
//! The detector keeps a rolling background image and measures how much of the new frame differs
//! from that background. It scores both whole-frame change and localized tile change so distant
//! people or small moving objects can still trigger motion without requiring huge scene changes.

use std::time::{Duration, Instant};

use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};

use crate::{config::MotionDetectionConfig, ffmpeg::VideoFrame};

#[derive(Debug, Clone, Copy)]
pub struct MotionEvent {
    pub motion_ratio: f32,
    pub local_motion_ratio: f32,
}

pub struct MotionDetector {
    pixel_difference_threshold: u8,
    motion_ratio_threshold: f32,
    local_motion_ratio_threshold: f32,
    local_motion_consecutive_frames: u32,
    background_alpha: f32,
    cooldown: Duration,
    background: Option<Vec<f32>>,
    last_event_at: Option<Instant>,
    local_motion_streak: u32,
}

impl MotionDetector {
    pub fn new(settings: &MotionDetectionConfig) -> Self {
        Self {
            pixel_difference_threshold: settings.pixel_difference_threshold,
            motion_ratio_threshold: settings.motion_ratio_threshold,
            local_motion_ratio_threshold: settings.local_motion_ratio_threshold,
            local_motion_consecutive_frames: settings.local_motion_consecutive_frames,
            background_alpha: settings.background_alpha,
            cooldown: Duration::from_secs(settings.event_cooldown_seconds),
            background: None,
            last_event_at: None,
            local_motion_streak: 0,
        }
    }

    pub fn analyze(&mut self, frame: &VideoFrame) -> Option<MotionEvent> {
        // Motion detection works on grayscale because it is cheaper and color is not important for
        // simple frame-difference math.
        let grayscale = rgb_to_luma(&frame.rgb);

        if self.background.is_none() {
            // The very first frame becomes the starting background reference.
            self.background = Some(grayscale.iter().map(|value| f32::from(*value)).collect());
            return None;
        }

        let background = self.background.as_mut()?;
        let mut changed_pixels = 0_usize;
        let frame_width = usize::try_from(frame.width).ok()?;
        let frame_height = usize::try_from(frame.height).ok()?;
        let tile_width = usize::max(1, frame_width.div_ceil(10));
        let tile_height = usize::max(1, frame_height.div_ceil(10));
        let tiles_x = frame_width.div_ceil(tile_width);
        let tiles_y = frame_height.div_ceil(tile_height);
        let mut tile_changed_counts = vec![0_usize; tiles_x * tiles_y];
        let mut tile_total_counts = vec![0_usize; tiles_x * tiles_y];

        for row in 0..frame_height {
            let tile_y = row / tile_height;
            let row_start = row * frame_width;
            let row_end = row_start + frame_width;

            for (column, (pixel, baseline)) in grayscale[row_start..row_end]
                .iter()
                .zip(background[row_start..row_end].iter_mut())
                .enumerate()
            {
                let tile_index = tile_y * tiles_x + (column / tile_width);
                tile_total_counts[tile_index] += 1;

                let pixel_value = f32::from(*pixel);
                let difference = (*baseline - pixel_value).abs();
                if difference >= f32::from(self.pixel_difference_threshold) {
                    changed_pixels += 1;
                    tile_changed_counts[tile_index] += 1;
                }
                // Exponential smoothing lets the background adapt slowly to scene changes like dawn,
                // dusk, or a light being switched on, without forgetting the previous frame entirely.
                *baseline = (*baseline * (1.0 - self.background_alpha))
                    + (pixel_value * self.background_alpha);
            }
        }

        let motion_ratio = changed_pixels as f32 / grayscale.len() as f32;
        // The local-tile score helps catch small moving subjects that would disappear inside the
        // whole-frame ratio when the camera sees a wide scene.
        let local_motion_ratio = tile_changed_counts
            .iter()
            .zip(tile_total_counts.iter())
            .filter_map(|(changed, total)| {
                if *total == 0 {
                    None
                } else {
                    Some(*changed as f32 / *total as f32)
                }
            })
            .fold(0.0_f32, f32::max);

        if local_motion_ratio >= self.local_motion_ratio_threshold {
            self.local_motion_streak = self.local_motion_streak.saturating_add(1);
        } else {
            self.local_motion_streak = 0;
        }

        let local_motion_triggered =
            self.local_motion_streak >= self.local_motion_consecutive_frames;

        if motion_ratio < self.motion_ratio_threshold && !local_motion_triggered {
            return None;
        }

        let now = Instant::now();
        if let Some(last_event_at) = self.last_event_at
            && now.duration_since(last_event_at) < self.cooldown
        {
            return None;
        }

        self.last_event_at = Some(now);
        Some(MotionEvent {
            motion_ratio,
            local_motion_ratio,
        })
    }
}

pub fn encode_snapshot_jpeg(frame: &VideoFrame, quality: u8) -> Result<Vec<u8>, image::ImageError> {
    // MQTT payloads are much smaller with JPEG snapshots than with raw RGB frame bytes.
    let mut output = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut output, quality);
    encoder.encode(
        &frame.rgb,
        frame.width,
        frame.height,
        ExtendedColorType::Rgb8,
    )?;
    Ok(output)
}

fn rgb_to_luma(rgb: &[u8]) -> Vec<u8> {
    let mut grayscale = Vec::with_capacity(rgb.len() / 3);

    for chunk in rgb.chunks_exact(3) {
        // Integer luma math keeps the conversion fast and allocation-friendly.
        let r = u16::from(chunk[0]);
        let g = u16::from(chunk[1]);
        let b = u16::from(chunk[2]);
        let luma = (77 * r + 150 * g + 29 * b) >> 8;
        grayscale.push(luma as u8);
    }

    grayscale
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::{MotionDetector, encode_snapshot_jpeg};
    use crate::{config::MotionDetectionConfig, ffmpeg::VideoFrame};

    fn build_frame(width: u32, height: u32, value: u8) -> VideoFrame {
        let mut rgb = Vec::new();
        for _ in 0..(width * height) {
            rgb.extend([value, value, value]);
        }

        VideoFrame {
            index: 0,
            captured_at: SystemTime::now(),
            width,
            height,
            rgb,
        }
    }

    #[test]
    fn first_frame_warms_background() {
        let mut detector = MotionDetector::new(&MotionDetectionConfig::default());
        let frame = build_frame(4, 4, 10);

        assert!(detector.analyze(&frame).is_none());
    }

    #[test]
    fn detects_large_scene_change() {
        let mut detector = MotionDetector::new(&MotionDetectionConfig {
            motion_ratio_threshold: 0.25,
            local_motion_ratio_threshold: 0.8,
            local_motion_consecutive_frames: 1,
            pixel_difference_threshold: 10,
            event_cooldown_seconds: 0,
            ..MotionDetectionConfig::default()
        });
        let still = build_frame(4, 4, 0);
        let motion = build_frame(4, 4, 255);

        assert!(detector.analyze(&still).is_none());
        assert!(detector.analyze(&motion).is_some());
    }

    #[test]
    fn cooldown_suppresses_duplicate_events() {
        let mut detector = MotionDetector::new(&MotionDetectionConfig {
            motion_ratio_threshold: 0.25,
            local_motion_ratio_threshold: 0.8,
            local_motion_consecutive_frames: 1,
            pixel_difference_threshold: 10,
            event_cooldown_seconds: 60,
            ..MotionDetectionConfig::default()
        });
        let still = build_frame(4, 4, 0);
        let motion = build_frame(4, 4, 255);

        assert!(detector.analyze(&still).is_none());
        assert!(detector.analyze(&motion).is_some());
        assert!(detector.analyze(&still).is_none());
        assert!(detector.analyze(&motion).is_none());
    }

    #[test]
    fn detects_small_localized_motion_cluster() {
        let mut detector = MotionDetector::new(&MotionDetectionConfig {
            motion_ratio_threshold: 0.2,
            local_motion_ratio_threshold: 0.2,
            local_motion_consecutive_frames: 2,
            pixel_difference_threshold: 10,
            event_cooldown_seconds: 0,
            ..MotionDetectionConfig::default()
        });
        let still = build_frame(20, 20, 0);
        let mut localized = build_frame(20, 20, 0);

        for row in 0..4 {
            for column in 0..4 {
                let index = ((row * 20) + column) as usize * 3;
                localized.rgb[index] = 255;
                localized.rgb[index + 1] = 255;
                localized.rgb[index + 2] = 255;
            }
        }

        assert!(detector.analyze(&still).is_none());
        assert!(detector.analyze(&localized).is_none());
        assert!(detector.analyze(&localized).is_some());
    }

    #[test]
    fn ignores_sparse_noise_across_tiles() {
        let mut detector = MotionDetector::new(&MotionDetectionConfig {
            motion_ratio_threshold: 0.2,
            local_motion_ratio_threshold: 0.2,
            local_motion_consecutive_frames: 2,
            pixel_difference_threshold: 10,
            event_cooldown_seconds: 0,
            ..MotionDetectionConfig::default()
        });
        let still = build_frame(100, 100, 0);
        let mut sparse = build_frame(100, 100, 0);

        for index in (0..100).step_by(10) {
            let pixel = ((index * 100) + index) as usize * 3;
            sparse.rgb[pixel] = 255;
            sparse.rgb[pixel + 1] = 255;
            sparse.rgb[pixel + 2] = 255;
        }

        assert!(detector.analyze(&still).is_none());
        assert!(detector.analyze(&sparse).is_none());
    }

    #[test]
    fn single_local_spike_does_not_trigger_without_streak() {
        let mut detector = MotionDetector::new(&MotionDetectionConfig {
            motion_ratio_threshold: 0.2,
            local_motion_ratio_threshold: 0.2,
            local_motion_consecutive_frames: 3,
            pixel_difference_threshold: 10,
            event_cooldown_seconds: 0,
            ..MotionDetectionConfig::default()
        });
        let still = build_frame(20, 20, 0);
        let mut localized = build_frame(20, 20, 0);

        for row in 0..4 {
            for column in 0..4 {
                let index = ((row * 20) + column) as usize * 3;
                localized.rgb[index] = 255;
                localized.rgb[index + 1] = 255;
                localized.rgb[index + 2] = 255;
            }
        }

        assert!(detector.analyze(&still).is_none());
        assert!(detector.analyze(&localized).is_none());
        assert!(detector.analyze(&still).is_none());
    }

    #[test]
    fn encodes_jpeg_snapshot() {
        let frame = build_frame(8, 8, 64);
        let jpeg = match encode_snapshot_jpeg(&frame, 80) {
            Ok(bytes) => bytes,
            Err(error) => panic!("expected jpeg encoding to succeed, got {error}"),
        };

        assert!(!jpeg.is_empty());
    }
}
