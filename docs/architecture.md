# Architecture Notes

## Selected tooling

- `ffmpeg` CLI handles decoding, scaling, FPS throttling, and protocol support without pulling OpenCV into the binary.
- `rumqttc` provides a small MQTT client with a dedicated network loop.
- Standard library threads keep the runtime model simple: one thread ingests frames, one analyzes motion, and MQTT runs on its own worker threads.
- Docker packages the binary plus `ffmpeg` so local and containerized runs behave the same way.

## Detection approach

- Frames are downscaled to a fixed RGB output size in `ffmpeg`.
- `src/motion.rs` converts each frame to grayscale and compares it against a rolling background model.
- A motion event is emitted when the changed-pixel ratio crosses `motion_ratio_threshold` and the cooldown window has elapsed.
- The same frame is JPEG-encoded and embedded in the MQTT payload as Base64.

## Service boundaries

- `src/config.rs`: CLI parsing, TOML loading, and validation.
- `src/ffmpeg.rs`: ffmpeg argument construction and frame streaming.
- `src/motion.rs`: background model, motion scoring, and JPEG snapshot encoding.
- `src/mqtt.rs`: MQTT startup, publishing, and event-loop management.
- `src/app.rs`: bootstrap, retry logic, shutdown handling, and payload assembly.
- `scripts/container-entrypoint.sh`: converts `CAMWATCH_INPUT` into the CLI arguments expected by the binary.

## Practical trade-offs

- Using the system `ffmpeg` binary keeps the Rust dependency tree lighter than OpenCV while still supporting RTSP well.
- Publishing JSON plus Base64 snapshots is easy for downstream consumers, though raw binary topics would be smaller if that becomes necessary later.
- Fixed-size analysis frames trade some fidelity for predictable CPU and memory usage.
- The container image is not scratch-minimal because it intentionally carries the system `ffmpeg` runtime.
