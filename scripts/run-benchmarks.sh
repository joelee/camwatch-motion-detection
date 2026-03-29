#!/usr/bin/env bash
set -euo pipefail

FIXTURES="tests/video"
RUNS="3"
FRAME_RATE="5"
DETECT_WIDTH="320"
DETECT_HEIGHT="180"
OUTPUT_DIR="benchmark-results"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --fixtures)
      FIXTURES="$2"
      shift 2
      ;;
    --runs)
      RUNS="$2"
      shift 2
      ;;
    --frame-rate)
      FRAME_RATE="$2"
      shift 2
      ;;
    --detect-width)
      DETECT_WIDTH="$2"
      shift 2
      ;;
    --detect-height)
      DETECT_HEIGHT="$2"
      shift 2
      ;;
    --output-dir)
      OUTPUT_DIR="$2"
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

mkdir -p "$OUTPUT_DIR"
RESULT_FILE="$OUTPUT_DIR/benchmark-$(date +%Y%m%d-%H%M%S).txt"

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required to capture resource usage" >&2
  exit 1
fi

cargo build --release --bin benchmark_resolution

run_case() {
  local label="$1"
  shift

  python3 - "$label" "$@" <<'PY'
import resource
import subprocess
import sys

label = sys.argv[1]
cmd = sys.argv[2:]
result = subprocess.run(cmd, text=True, capture_output=True)
usage = resource.getrusage(resource.RUSAGE_CHILDREN)

print(f"=== {label} ===")
print("command:", " ".join(cmd))
print(result.stdout, end="")
print(
    f"resource user_cpu_s={usage.ru_utime:.4f} sys_cpu_s={usage.ru_stime:.4f} max_rss_kb={usage.ru_maxrss}"
)
if result.stderr:
    print(result.stderr, end="", file=sys.stderr)
sys.exit(result.returncode)
PY
}

{
  echo "benchmark_date=$(date --iso-8601=seconds)"
  echo "host=$(uname -a)"
  echo "rustc=$(rustc --version)"
  echo "cargo=$(cargo --version)"
  echo "ffmpeg=$(ffmpeg -version | sed -n '1p')"
  echo "fixtures=$FIXTURES"
  echo "runs=$RUNS"
  echo "frame_rate=$FRAME_RATE"
  echo "detect_width=$DETECT_WIDTH"
  echo "detect_height=$DETECT_HEIGHT"
  echo

  run_case \
    "single detect ${DETECT_WIDTH}x${DETECT_HEIGHT} output ${DETECT_WIDTH}x${DETECT_HEIGHT}" \
    target/release/benchmark_resolution \
    --mode single \
    --fixtures "$FIXTURES" \
    --detect-width "$DETECT_WIDTH" \
    --detect-height "$DETECT_HEIGHT" \
    --output-width "$DETECT_WIDTH" \
    --output-height "$DETECT_HEIGHT" \
    --frame-rate "$FRAME_RATE" \
    --runs "$RUNS"
  echo

  run_case \
    "aspect16x9 matrix detect ${DETECT_WIDTH}x${DETECT_HEIGHT}" \
    target/release/benchmark_resolution \
    --mode aspect16x9 \
    --fixtures "$FIXTURES" \
    --detect-width "$DETECT_WIDTH" \
    --detect-height "$DETECT_HEIGHT" \
    --frame-rate "$FRAME_RATE" \
    --runs "$RUNS"
  echo

  run_case \
    "single detect ${DETECT_WIDTH}x${DETECT_HEIGHT} output source" \
    target/release/benchmark_resolution \
    --mode single \
    --fixtures "$FIXTURES" \
    --detect-width "$DETECT_WIDTH" \
    --detect-height "$DETECT_HEIGHT" \
    --frame-rate "$FRAME_RATE" \
    --runs "$RUNS"
} | tee "$RESULT_FILE"

echo
echo "saved benchmark report to $RESULT_FILE"
