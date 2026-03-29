# Configuration

All tunable runtime settings live in `camwatch.toml` under `[motion_detection]`.

## Settings

| Setting | Purpose | Default |
| --- | --- | --- |
| `frame_width` | Output frame width from `ffmpeg` before motion analysis | `320` |
| `frame_height` | Output frame height from `ffmpeg` before motion analysis | `180` |
| `output_frame_width` | Optional snapshot/output frame width; defaults to source width when omitted | source width |
| `output_frame_height` | Optional snapshot/output frame height; defaults to source height when omitted | source height |
| `frame_rate` | Frames per second sampled from the source | `5` |
| `pixel_difference_threshold` | Per-pixel grayscale delta required to count as changed | `20` |
| `motion_ratio_threshold` | Fraction of changed pixels required to emit a motion event | `0.015` |
| `local_motion_ratio_threshold` | Fraction of changed pixels required within a local tile to emit a motion event | `0.095` |
| `local_motion_consecutive_frames` | Number of consecutive locally-active frames required before a local motion session starts | `4` |
| `motion_end_grace_seconds` | Quiet time required before a motion session is considered finished | `1` |
| `motion_snapshot_delay_seconds` | Snapshot offset used once motion lasts at least this long | `5` |
| `long_motion_snapshot_interval_seconds` | Extra snapshot interval for very long motion sessions | `30` |
| `background_alpha` | Rolling background update factor | `0.08` |
| `event_cooldown_seconds` | Cooldown before a new motion session can start after one ends | `10` |
| `snapshot_jpeg_quality` | JPEG quality for MQTT snapshots | `80` |
| `output_directory` | Optional directory for `motion-YYYYMMDD-HHMMSS.[jpg|toml]` files | `""` |
| `mqtt_host` | MQTT broker hostname | `127.0.0.1` |
| `mqtt_port` | MQTT broker port | `1883` |
| `mqtt_topic` | Topic used for motion events | `camwatch/motion` |
| `mqtt_client_id` | MQTT client id | `camwatch-motion-detection` |
| `mqtt_username` | Optional MQTT username | `""` |
| `mqtt_password` | Optional MQTT password | `""` |
| `mqtt_qos` | MQTT QoS level | `1` |
| `mqtt_keep_alive_seconds` | MQTT keep-alive interval | `30` |
| `rtsp_transport` | RTSP transport for ffmpeg, `tcp` or `udp` | `tcp` |
| `rtsp_retry_delay_seconds` | Delay between reconnect attempts | `5` |
| `rtsp_max_retries` | Retry count after a disconnect | `12` |

## Motion sensitivity tuning

The main sensitivity controls already live in `[motion_detection]`:

- `pixel_difference_threshold`: lower values make smaller per-pixel changes count as motion.
- `motion_ratio_threshold`: lower values make a smaller changed area trigger an event.
- `local_motion_ratio_threshold`: lower values make small localized motion, like a distant person, easier to detect.
- `local_motion_consecutive_frames`: lower values react faster to small local motion, while higher values suppress one-frame flicker and compression noise.
- `motion_end_grace_seconds`: higher values keep a motion session open longer before it is considered finished.
- `background_alpha`: higher values adapt to scene changes faster; lower values keep a steadier background model.
- `frame_rate`: higher sampling can catch shorter bursts of motion, at the cost of more CPU.

## Detection vs output resolution

- `frame_width` and `frame_height` only control the internal motion-analysis resolution.
- `output_frame_width` and `output_frame_height` control the resolution of saved `.jpg` files and MQTT snapshots.
- If `output_frame_width` and `output_frame_height` are omitted, the app keeps the source video resolution for output snapshots.
- If you set one output dimension, you must set both.

In practice, `local_motion_ratio_threshold`, `motion_ratio_threshold`, and `pixel_difference_threshold` are the primary knobs for "more sensitive" versus "less sensitive" behavior.

## Output targets

- At least one output must be configured: MQTT, `output_directory`, or both.
- MQTT is considered enabled when both `mqtt_host` and `mqtt_topic` are non-empty.
- File output is enabled when `output_directory` is set to a non-empty path.
- If file output is enabled, the app writes `motion-YYYYMMDD-HHMMSS.jpg` and `motion-YYYYMMDD-HHMMSS.toml` for each saved snapshot.
- For motion shorter than `motion_snapshot_delay_seconds`, the saved snapshot is chosen from the middle of the session.
- For longer motion, the first saved snapshot is taken at `motion_snapshot_delay_seconds`.
- For sessions longer than `long_motion_snapshot_interval_seconds`, extra snapshots are saved every `long_motion_snapshot_interval_seconds`.

## CLI

- `--input`: required RTSP URL or local video file path.
- `--config`: optional TOML path.

If `--config` is omitted, the app searches for a config file in this order:

1. `${HOME}/.config/camwatch/camwatch.toml`
2. `${HOME}/.config/camwatch.toml`
3. `/etc/camwatch.toml`

The repository's top-level `camwatch.toml` is intended as the default example file you can copy into one of those runtime locations.

## Containers

- The Docker entrypoint expects `CAMWATCH_INPUT` and forwards it to `--input`.
- `camwatch.toml` is read from `/app/camwatch.toml` inside the container unless you override the entrypoint or command.
- The included `compose.yaml` mounts `docker/camwatch.compose.toml`, where `mqtt_host = "mqtt"` matches the bundled Mosquitto service.
