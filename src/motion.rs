//! Lightweight motion scoring.
//!
//! The detector keeps a rolling background image and measures how much of the new frame differs
//! from that background. It scores both whole-frame change and localized tile change so distant
//! people or small moving objects can still trigger motion without requiring huge scene changes.

use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};

use crate::{config::MotionDetectionConfig, ffmpeg::VideoFrame};

#[derive(Debug, Clone, Copy)]
pub struct MotionEvent {
    pub motion_ratio: f32,
    pub local_motion_ratio: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct MotionAnalysis {
    pub motion_ratio: f32,
    pub local_motion_ratio: f32,
    pub global_triggered: bool,
    pub local_triggered: bool,
}

impl MotionAnalysis {
    pub fn is_motion_active(self) -> bool {
        self.global_triggered || self.local_triggered
    }

    pub fn event(self) -> MotionEvent {
        MotionEvent {
            motion_ratio: self.motion_ratio,
            local_motion_ratio: self.local_motion_ratio,
        }
    }
}

pub struct MotionDetector {
    detection_width: u32,
    detection_height: u32,
    pixel_difference_threshold: u8,
    motion_ratio_threshold: f32,
    local_motion_ratio_threshold: f32,
    background_alpha: f32,
    background: Option<Vec<f32>>,
}

impl MotionDetector {
    pub fn new(settings: &MotionDetectionConfig) -> Self {
        Self {
            detection_width: settings.frame_width,
            detection_height: settings.frame_height,
            pixel_difference_threshold: settings.pixel_difference_threshold,
            motion_ratio_threshold: settings.motion_ratio_threshold,
            local_motion_ratio_threshold: settings.local_motion_ratio_threshold,
            background_alpha: settings.background_alpha,
            background: None,
        }
    }

    pub fn analyze(&mut self, frame: &VideoFrame) -> MotionAnalysis {
        // Motion detection works on grayscale because it is cheaper and color is not important for
        // simple frame-difference math.
        let grayscale = sample_rgb_to_luma(
            &frame.rgb,
            frame.width,
            frame.height,
            self.detection_width,
            self.detection_height,
        );

        if self.background.is_none() {
            // The very first frame becomes the starting background reference.
            self.background = Some(grayscale.iter().map(|value| f32::from(*value)).collect());
            return MotionAnalysis {
                motion_ratio: 0.0,
                local_motion_ratio: 0.0,
                global_triggered: false,
                local_triggered: false,
            };
        }

        let Some(background) = self.background.as_mut() else {
            return MotionAnalysis {
                motion_ratio: 0.0,
                local_motion_ratio: 0.0,
                global_triggered: false,
                local_triggered: false,
            };
        };

        let Ok(frame_width) = usize::try_from(self.detection_width) else {
            return MotionAnalysis {
                motion_ratio: 0.0,
                local_motion_ratio: 0.0,
                global_triggered: false,
                local_triggered: false,
            };
        };
        let Ok(frame_height) = usize::try_from(self.detection_height) else {
            return MotionAnalysis {
                motion_ratio: 0.0,
                local_motion_ratio: 0.0,
                global_triggered: false,
                local_triggered: false,
            };
        };

        let mut changed_pixels = 0_usize;
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

        MotionAnalysis {
            motion_ratio,
            local_motion_ratio,
            global_triggered: motion_ratio >= self.motion_ratio_threshold,
            local_triggered: local_motion_ratio >= self.local_motion_ratio_threshold,
        }
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

fn sample_rgb_to_luma(
    rgb: &[u8],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> Vec<u8> {
    let source_width = usize::try_from(source_width).unwrap_or(0);
    let source_height = usize::try_from(source_height).unwrap_or(0);
    let target_width = usize::try_from(target_width).unwrap_or(0);
    let target_height = usize::try_from(target_height).unwrap_or(0);

    if source_width == 0 || source_height == 0 || target_width == 0 || target_height == 0 {
        return Vec::new();
    }

    let mut grayscale = Vec::with_capacity(target_width * target_height);

    for target_y in 0..target_height {
        let source_y = target_y * source_height / target_height;
        let source_y_end = usize::max(source_y + 1, (target_y + 1) * source_height / target_height);
        for target_x in 0..target_width {
            let source_x = target_x * source_width / target_width;
            let source_x_end =
                usize::max(source_x + 1, (target_x + 1) * source_width / target_width);
            let mut luma_sum = 0_u64;
            let mut pixel_count = 0_u64;

            for y in source_y..source_y_end {
                for x in source_x..source_x_end {
                    let index = ((y * source_width) + x) * 3;
                    let r = u16::from(rgb[index]);
                    let g = u16::from(rgb[index + 1]);
                    let b = u16::from(rgb[index + 2]);
                    // Integer luma math keeps the conversion fast and allocation-friendly.
                    let luma = (77 * r + 150 * g + 29 * b) >> 8;
                    luma_sum += u64::from(luma);
                    pixel_count += 1;
                }
            }

            grayscale.push((luma_sum / pixel_count) as u8);
        }
    }

    grayscale
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::{MotionDetector, encode_snapshot_jpeg, sample_rgb_to_luma};
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

        assert!(!detector.analyze(&frame).is_motion_active());
    }

    #[test]
    fn detects_large_scene_change() {
        let mut detector = MotionDetector::new(&MotionDetectionConfig {
            frame_width: 4,
            frame_height: 4,
            motion_ratio_threshold: 0.25,
            local_motion_ratio_threshold: 0.8,
            pixel_difference_threshold: 10,
            ..MotionDetectionConfig::default()
        });
        let still = build_frame(4, 4, 0);
        let motion = build_frame(4, 4, 255);

        assert!(!detector.analyze(&still).is_motion_active());
        let analysis = detector.analyze(&motion);
        assert!(analysis.is_motion_active());
        assert!(analysis.global_triggered);
    }

    #[test]
    fn detects_small_localized_motion_cluster() {
        let mut detector = MotionDetector::new(&MotionDetectionConfig {
            frame_width: 20,
            frame_height: 20,
            motion_ratio_threshold: 0.2,
            local_motion_ratio_threshold: 0.2,
            pixel_difference_threshold: 10,
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

        assert!(!detector.analyze(&still).is_motion_active());
        let analysis = detector.analyze(&localized);
        assert!(analysis.is_motion_active());
        assert!(analysis.local_triggered);
    }

    #[test]
    fn ignores_sparse_noise_across_tiles() {
        let mut detector = MotionDetector::new(&MotionDetectionConfig {
            frame_width: 100,
            frame_height: 100,
            motion_ratio_threshold: 0.2,
            local_motion_ratio_threshold: 0.2,
            pixel_difference_threshold: 10,
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

        assert!(!detector.analyze(&still).is_motion_active());
        assert!(!detector.analyze(&sparse).is_motion_active());
    }

    #[test]
    fn analysis_event_carries_ratios() {
        let mut detector = MotionDetector::new(&MotionDetectionConfig {
            frame_width: 4,
            frame_height: 4,
            motion_ratio_threshold: 0.2,
            local_motion_ratio_threshold: 0.2,
            pixel_difference_threshold: 10,
            ..MotionDetectionConfig::default()
        });
        let still = build_frame(4, 4, 0);
        let motion = build_frame(4, 4, 255);

        let _ = detector.analyze(&still);
        let event = detector.analyze(&motion).event();

        assert!(event.motion_ratio > 0.0);
        assert!(event.local_motion_ratio > 0.0);
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

    #[test]
    fn samples_source_frame_down_to_detection_resolution() {
        let rgb = vec![
            0, 0, 0, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255,
            0, 0, 0,
        ];

        let grayscale = sample_rgb_to_luma(&rgb, 4, 2, 2, 1);

        assert_eq!(grayscale.len(), 2);
        assert_eq!(grayscale[0], 127);
        assert_eq!(grayscale[1], 127);
    }
}
