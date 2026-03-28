# Docker Notes

## Files

- `Dockerfile`: multi-stage build for the Rust binary plus an `ffmpeg` runtime image.
- `compose.yaml`: local development stack with the app and Mosquitto.
- `docker/camwatch.compose.toml`: compose-specific runtime config that points MQTT at the `mqtt` service.
- `scripts/container-entrypoint.sh`: validates `CAMWATCH_INPUT` and starts the CLI.
- `docker/mosquitto/mosquitto.conf`: simple local broker config for Compose.

## Environment

- `CAMWATCH_INPUT` is required in container runs.
- `RUST_LOG` is optional and defaults to standard tracing behavior.

## Compose defaults

- The app reads `docker/camwatch.compose.toml` in Compose so MQTT resolves to the `mqtt` service name.
- The Mosquitto service is reachable at hostname `mqtt` from the app container.
- If you use a local file instead of RTSP, mount that file or directory into the container and point `CAMWATCH_INPUT` at the mounted path.
