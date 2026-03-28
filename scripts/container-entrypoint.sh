#!/bin/sh
set -eu

if [ -z "${CAMWATCH_INPUT:-}" ]; then
  echo "CAMWATCH_INPUT must be set to an RTSP URL or mounted file path" >&2
  exit 1
fi

exec /usr/local/bin/camwatch-motion-detection --config /app/camwatch.toml --input "$CAMWATCH_INPUT"
