# Configuration

All tunable runtime settings live in `camwatch.toml` under `[motion_detection]`.

## Settings

| Setting | Purpose | Default |
| --- | --- | --- |
| `frame_width` | Output frame width from `ffmpeg` before motion analysis | `320` |
| `frame_height` | Output frame height from `ffmpeg` before motion analysis | `180` |
| `frame_rate` | Frames per second sampled from the source | `5` |
| `pixel_difference_threshold` | Per-pixel grayscale delta required to count as changed | `20` |
| `motion_ratio_threshold` | Fraction of changed pixels required to emit a motion event | `0.015` |
| `local_motion_ratio_threshold` | Fraction of changed pixels required within a local tile to emit a motion event | `0.095` |
| `local_motion_consecutive_frames` | Number of consecutive locally-active frames required before a local trigger fires | `4` |
| `background_alpha` | Rolling background update factor | `0.08` |
| `event_cooldown_seconds` | Minimum time between emitted events | `10` |
| `snapshot_jpeg_quality` | JPEG quality for MQTT snapshots | `80` |
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
- `background_alpha`: higher values adapt to scene changes faster; lower values keep a steadier background model.
- `frame_rate`: higher sampling can catch shorter bursts of motion, at the cost of more CPU.

In practice, `local_motion_ratio_threshold`, `motion_ratio_threshold`, and `pixel_difference_threshold` are the primary knobs for "more sensitive" versus "less sensitive" behavior.

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
