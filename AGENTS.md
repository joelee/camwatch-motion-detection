# AGENTS.md

## Project purpose

This repository hosts a lightweight Rust CLI that reads an RTSP stream or video file, detects motion in near real time, and publishes motion events with JPEG snapshots to MQTT.

## Recommended engineering workflow

1. Keep runtime tuning in `camwatch.toml` under `[motion_detection]`.
2. Preserve the thread split between frame ingestion, motion analysis, and MQTT publishing unless there is a measured reason to change it.
3. Prefer extending `src/motion.rs`, `src/ffmpeg.rs`, and `src/mqtt.rs` behind small helpers instead of growing `src/app.rs` into a monolith.
4. Run the full quality gate before proposing changes:
   - `cargo fmt --all`
   - `cargo clippy --all-targets --all-features -- -D warnings`
   - `cargo test --all-targets`
   - `pre-commit run --all-files`
5. Keep container files (`Dockerfile`, `compose.yaml`, `scripts/container-entrypoint.sh`) aligned with CLI and config changes.

## Coding conventions

- Avoid `unwrap` and `expect` in production paths.
- Keep motion thresholds explicit and tested when changing detection behavior.
- Prefer simple frame-difference math over heavyweight dependencies unless profiling shows a clear need.
- Document MQTT payload shape and config changes in `README.md` and `docs/`.
- Prefer comments that explain why a thread, channel, or algorithm exists over comments that restate syntax.
- Prefer unit tests for deterministic logic in `src/`, and use integration tests in `tests/` when the behavior depends on ffmpeg, video fixtures, or end-to-end output files.

## Operational notes

- The binary shells out to the system `ffmpeg` executable for decoding and scaling.
- File inputs are read in real time with `ffmpeg -re` and exit after the file is consumed.
- RTSP inputs reconnect using `rtsp_retry_delay_seconds` and `rtsp_max_retries`.
- MQTT publishes JSON payloads with a Base64-encoded JPEG snapshot on the configured topic.
- Docker Compose includes a local Mosquitto broker for development and smoke testing.
