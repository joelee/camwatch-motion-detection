# MQTT Payload

Motion events are published as JSON to the configured `mqtt_topic`.

## Example

```json
{
  "source": "rtsp://camera.local/live",
  "captured_at_epoch_ms": 1711459200000,
  "frame_index": 42,
  "motion_ratio": 0.1834,
  "frame_width": 320,
  "frame_height": 180,
  "snapshot_jpeg_base64": "..."
}
```

## Notes

- `snapshot_jpeg_base64` is the JPEG snapshot for the frame that triggered the event.
- `motion_ratio` is the fraction of pixels that exceeded `pixel_difference_threshold`.
- `frame_index` counts sampled frames after `ffmpeg` FPS limiting and scaling.
