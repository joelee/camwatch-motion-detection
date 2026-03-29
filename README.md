# CamWatch Motion Detection CLI

`camwatch-motion-detection` is a lightweight Rust CLI that reads an RTSP stream or a local video file, detects motion in near real time, and publishes MQTT events with JPEG snapshots.

It is designed to be a long-running companion service for CamWatch-style automation pipelines that need cheap motion triggers without a heavyweight CV stack.

## Why this stack

- `ffmpeg` handles RTSP, file decoding, scaling, and frame-rate limiting with mature codec support.
- Standard library threads keep the runtime model simple and efficient.
- `rumqttc`, `clap`, `serde`, `tracing`, `clippy`, and `rustfmt` keep the operational surface predictable.

## Features

- Accepts `--input` as either an RTSP URL or a local video file.
- Processes frames in real time from files via `ffmpeg -re`.
- Detects motion with a rolling grayscale background model.
- Publishes motion metadata plus a Base64-encoded JPEG snapshot to MQTT.
- Retries RTSP reconnects using `camwatch.toml` settings.

## Requirements

- Rust toolchain with Cargo.
- System `ffmpeg` executable available on `PATH`.
- Reachable MQTT broker.

## Quick start

1. Review and adjust `camwatch.toml`.
2. Run against a local file:

   ```bash
   cargo run -- --input /path/to/video.mp4
   ```

3. Run against RTSP:

   ```bash
   cargo run -- --input rtsp://camera.local/live
   ```

4. Override the config path if needed:

   ```bash
   cargo run -- --config ./custom.toml --input rtsp://camera.local/live
   ```

If `--config` is not provided, the app looks for a config file in this order:

1. `${HOME}/.config/camwatch/camwatch.toml`
2. `${HOME}/.config/camwatch.toml`
3. `/etc/camwatch.toml`

The repository's `camwatch.toml` remains the default example file you can copy into one of those locations.

## Docker

1. Build the image:

   ```bash
   docker build -t camwatch-motion-detection .
   ```

2. Run it by passing the input as an environment variable consumed by the container entrypoint:

   ```bash
   docker run --rm \
     -e CAMWATCH_INPUT=rtsp://camera.local/live \
     -v "$(pwd)/camwatch.toml:/app/camwatch.toml:ro" \
     camwatch-motion-detection
   ```

3. For local files, mount the media into the container and point `CAMWATCH_INPUT` at the mounted path:

   ```bash
   docker run --rm \
     -e CAMWATCH_INPUT=/data/video.mp4 \
     -v "$(pwd)/camwatch.toml:/app/camwatch.toml:ro" \
     -v "$(pwd)/samples:/data:ro" \
     camwatch-motion-detection
   ```

The runtime image installs `ffmpeg`, copies the compiled binary, and starts through `scripts/container-entrypoint.sh`.

## Compose

1. Set the required input:

   ```bash
   export CAMWATCH_INPUT=rtsp://camera.local/live
   ```

2. Start the bundled MQTT broker and motion detector:

   ```bash
   docker compose up --build
   ```

3. Stop the stack when finished:

   ```bash
   docker compose down
   ```

The included `compose.yaml` starts both `camwatch` and an `eclipse-mosquitto` broker, and mounts `docker/camwatch.compose.toml` so the app can reach the broker at hostname `mqtt`. For file inputs, mount your video directory into `./samples` or adjust the volume mapping in `compose.yaml`.

## Configuration

All runtime tuning lives in `camwatch.toml` under `[motion_detection]`.

Key settings:

| Setting | Default |
| --- | --- |
| `frame_width` | `320` |
| `frame_height` | `180` |
| `output_frame_width` | source width |
| `output_frame_height` | source height |
| `frame_rate` | `5` |
| `pixel_difference_threshold` | `20` |
| `motion_ratio_threshold` | `0.015` |
| `local_motion_ratio_threshold` | `0.095` |
| `motion_snapshot_delay_seconds` | `5` |
| `long_motion_snapshot_interval_seconds` | `30` |
| `output_directory` | `""` |
| `event_cooldown_seconds` | `10` |
| `mqtt_topic` | `camwatch/motion` |
| `rtsp_retry_delay_seconds` | `5` |
| `rtsp_max_retries` | `12` |

More detail lives in `docs/configuration.md`.

For outputs, configure at least one of these:

- MQTT by setting both `mqtt_host` and `mqtt_topic`
- File output by setting `output_directory`

When `output_directory` is enabled, each event writes:

- `motion-YYYYMMDD-HHMMSS.jpg`
- `motion-YYYYMMDD-HHMMSS.toml`

Snapshot timing follows the motion session:

- If the session ends before `motion_snapshot_delay_seconds`, the saved frame comes from the middle of the motion.
- If the session lasts longer, the first saved frame comes from `motion_snapshot_delay_seconds` after motion start.
- If the session continues beyond `long_motion_snapshot_interval_seconds`, additional snapshots are saved every `long_motion_snapshot_interval_seconds`.

Resolution behavior:

- `frame_width` and `frame_height` control the lightweight motion-analysis resolution.
- `output_frame_width` and `output_frame_height` control the saved/published snapshot size.
- If `output_frame_width` and `output_frame_height` are omitted, snapshots keep the source video resolution.

## MQTT payload

When MQTT is enabled, the app publishes JSON to the configured topic with motion metadata and a Base64-encoded JPEG snapshot.

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

More detail lives in `docs/mqtt-payload.md`.

## Benchmarking

- Use `cargo build --release --bin benchmark_resolution` to build the benchmark helper.
- The helper supports separate detection and output resolutions with `--detect-width`, `--detect-height`, `--output-width`, and `--output-height`.
- It also supports `--mode aspect16x9` to compare `320x180`, `640x360`, and `1280x720` style outputs in one run.
- Use `scripts/run-benchmarks.sh` on a target machine to build the benchmark, run the matrix, and save a timestamped report.
- Use `scripts/benchmark-report-to-markdown.py` to convert a saved benchmark report into Markdown tables.
- Benchmark instructions and sample results live in `docs/benchmarking.md`.

## Learning notes

- The code is split into small modules so a novice Rust reader can follow one concern at a time.
- Inline comments explain the less-obvious pieces such as bounded channels, rolling background updates, and the MQTT event loop.
- `src/main.rs` stays tiny; nearly all logic lives in the library crate so tests can call it more easily.

## RTSP behavior

- RTSP sessions stay alive until interrupted.
- On disconnect, the app retries every `rtsp_retry_delay_seconds` for `rtsp_max_retries` attempts.
- Local video files terminate naturally once the file has been streamed in real time.

## Quality gates

- Format: `cargo fmt --all`
- Lint: `cargo clippy --all-targets --all-features -- -D warnings`
- Tests: `cargo test --all-targets`
- Pre-commit: `pre-commit run --all-files`

## Testing

- Unit tests live in the Rust modules under `src/`.
- Integration tests live in `tests/` and use fixture videos from `tests/video/`.
- File-output integration tests write generated artifacts to `tests/output/`, which is gitignored.
- Detailed test documentation lives in `docs/testing.md`.

## Video fixture tests

- Add MP4 fixtures to `tests/video/`.
- Name files like `yard-12.mp4` when motion should begin at second `12`, or `garage-none.mp4` when no motion should be detected.
- The integration test reads fixture videos as fast as possible, not in real time, so the suite stays practical.
- If `tests/video/` has no MP4 files yet, the fixture test prints a skip message and passes.

## Repository guide

- `src/config.rs`: CLI parsing, TOML loading, and validation.
- `src/ffmpeg.rs`: frame ingestion through `ffmpeg`.
- `src/motion.rs`: grayscale background model and JPEG snapshots.
- `src/mqtt.rs`: MQTT publishing runtime.
- `src/output.rs`: shared formatting for MQTT payloads and on-disk event files.
- `src/session.rs`: motion session tracking and delayed snapshot selection.
- `src/app.rs`: startup, retries, and shutdown flow.
- `camwatch.toml`: runtime configuration under `[motion_detection]`.
- `tests/video_fixtures.rs`: end-to-end motion checks against MP4 fixtures in `tests/video/`.

## Documentation

For more details, see the following documentation:

- [Architecture](docs/architecture.md): processing architecture, module boundaries, trade-offs, and the motion pipeline diagram.
- [Benchmarking](docs/benchmarking.md): benchmark modes, helper scripts, and sample performance results.
- [Configuration](docs/configuration.md): complete `camwatch.toml` settings reference and tuning notes.
- [Docker](docs/docker.md): Docker and Compose files, environment variables, and container defaults.
- [MQTT Payload](docs/mqtt-payload.md): MQTT event payload fields and example JSON.
- [Testing](docs/testing.md): unit tests, integration tests, fixture naming, and test commands.


## License

This project is licensed under the Apache License 2.0. See `LICENSE`.
