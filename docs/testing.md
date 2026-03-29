# Testing

This project uses a mix of fast unit tests and fixture-driven integration tests.

## Test layers

- Unit tests live next to the Rust modules in `src/` under `#[cfg(test)]` blocks.
- Integration tests live in `tests/` and exercise larger slices of the application with real video fixtures.
- The quality gate also runs formatting, linting, and the configured pre-commit hooks.

## Unit tests

The unit tests focus on logic that can be validated without running the full CLI.

### `src/config.rs`

- CLI and TOML parsing
- default config-path search order
- config validation rules
- output target validation
- output-dimension validation

### `src/ffmpeg.rs`

- ffmpeg argument construction
- RTSP transport flags
- file realtime toggling for production vs tests
- output-dimension scaling arguments

### `src/motion.rs`

- grayscale conversion and downsampling
- global and local motion scoring
- sparse-noise rejection
- JPEG snapshot encoding

### `src/session.rs`

- motion session start and end tracking
- midpoint selection for short motion
- delayed snapshot selection for longer motion
- periodic snapshots for long-running motion
- event-based session notifications used by the processor

### `src/output.rs`

- MQTT payload serialization
- sidecar TOML generation
- output filename conventions

## Integration tests

Integration tests use the MP4 fixtures in `tests/video/`.

### `tests/video_fixtures.rs`

- runs the real ffmpeg-to-detector path
- validates expected motion start timing from fixture filenames
- supports names like `name-12.mp4` and `name-none.mp4`

### `tests/output_files.rs`

- runs motion detection against the fixture videos
- writes generated output into `tests/output/`
- verifies `.jpg` and `.toml` files are created for motion fixtures
- verifies `-none` fixtures do not create output files

### `tests/output_dimensions.rs`

- verifies source-resolution output when `output_frame_width` and `output_frame_height` are omitted
- verifies configured snapshot dimensions when output dimensions are set explicitly

### `tests/common/mod.rs`

- shared fixture discovery helpers
- shared fixture naming rules
- shared test configuration defaults

## Video fixtures

- Place fixture videos in `tests/video/`.
- Use `name-12.mp4` when motion should begin near second `12`.
- Use `name-none.mp4` when no motion should be detected.
- Keep fixtures short enough that the test suite stays practical.

Generated output from integration tests is written to `tests/output/`, which is ignored by git.

## Running tests

Run the full test suite:

```bash
cargo test --all-targets
```

Run a single integration test file:

```bash
cargo test --test video_fixtures -- --nocapture
cargo test --test output_files -- --nocapture
cargo test --test output_dimensions -- --nocapture
```

## Full quality gate

Before shipping changes, run:

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
pre-commit run --all-files
```

## Notes

- The integration tests depend on `ffmpeg` being available on `PATH`.
- If `tests/video/` has no MP4 files yet, the fixture-based integration tests print a skip message and pass.
- Benchmark tooling is documented separately in `docs/benchmarking.md`.
