# MQTT Payload

When MQTT is enabled, motion events are published as JSON to the configured `mqtt_topic`.

## Example

```json
{
  "source": "rtsp://camera.local/live",
  "captured_at_epoch_ms": 1711459200000,
  "motion_started_at_epoch_ms": 1711459195000,
  "motion_ended_at_epoch_ms": 1711459203000,
  "motion_duration_ms": 8000,
  "frame_index": 42,
  "motion_started_frame_index": 17,
  "motion_ended_frame_index": 57,
  "motion_ratio": 0.1834,
  "local_motion_ratio": 0.2217,
  "frame_width": 1920,
  "frame_height": 1080,
  "snapshot_jpeg_base64": "..."
}
```

## Notes

- `snapshot_jpeg_base64` is the JPEG snapshot for the frame that triggered the event.
- `motion_started_at_epoch_ms` and `motion_ended_at_epoch_ms` describe the full motion session that the snapshot belongs to.
- `motion_duration_ms` is the session duration in milliseconds.
- `motion_ratio` is the fraction of pixels that exceeded `pixel_difference_threshold`.
- `local_motion_ratio` is the strongest changed-tile ratio for the frame.
- `frame_index` counts sampled frames after `ffmpeg` FPS limiting.
- `frame_width` and `frame_height` describe the snapshot/output image dimensions, which can be larger than the motion-analysis dimensions.
