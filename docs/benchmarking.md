# Benchmarking

This project includes a small benchmark binary for comparing motion-detection cost at different
analysis and output resolutions.

## What it measures

- End-to-end wall time: `ffmpeg` decode, resize, piping, and Rust-side analysis.
- Detector-only time: time spent inside `MotionDetector::analyze`.
- Effective throughput in frames per second.

## Build

```bash
cargo build --release --bin benchmark_resolution
```

## Run

Example commands with motion detection fixed at `320x180`:

```bash
target/release/benchmark_resolution --mode single --detect-width 320 --detect-height 180 --output-width 320 --output-height 180 --runs 3
target/release/benchmark_resolution --mode aspect16x9 --detect-width 320 --detect-height 180 --runs 3
target/release/benchmark_resolution --mode single --detect-width 320 --detect-height 180 --runs 3
```

- `--mode single` benchmarks exactly one detection/output combination.
- `--mode aspect16x9` runs an aspect-ratio-consistent matrix for `320x180`, `640x360`, and `1280x720` outputs.
- Omitting `--output-width` and `--output-height` in `single` mode uses the source video resolution for snapshots and output frames.

## Helper script

For production machines such as Raspberry Pi, use:

```bash
scripts/run-benchmarks.sh --runs 3
```

The script:

- builds `benchmark_resolution` in release mode
- records system information and ffmpeg version
- runs a small benchmark matrix with fixed detection resolution
- writes a timestamped report into `benchmark-results/`

## Memory and CPU

To capture rough resource numbers on Linux, wrap each command with a helper such as Python's
`resource.getrusage` or an equivalent system tool available on your machine.

## Sample results

These measurements were taken from the current development machine using the three fixtures in
`tests/video/`, all sampled at `5 fps`, with detection fixed at `320x180`.

| Detection | Output | End-to-end ms/frame | End-to-end fps | Detector ms/frame | Detector fps | Max RSS |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| `320x180` | `320x180` | `3.5867` | `278.81` | `0.4827` | `2071.63` | about `202 MB` |
| `320x180` | `640x360` | `3.6369` | `274.96` | `0.8784` | `1138.48` | about `227 MB` |
| `320x180` | `1280x720` | `4.0944` | `244.24` | `2.2749` | `439.58` | about `291 MB` |
| `320x180` | source (`1920x1080`) | `4.0226` | `248.59` | `3.2822` | `304.67` | about `411 MB` |

## Reading the results

- Detector cost rises as output resolution rises because the detector now samples from the larger
  output frame down to the smaller analysis frame.
- The `aspect16x9` mode gives a cleaner apples-to-apples comparison than a `640x480` padded output.
- Keeping output at source resolution preserves image detail, but it is noticeably heavier on CPU
  and memory than writing smaller snapshots.
- A `320x180` detection frame remains a practical default when you want low-cost motion analysis.
