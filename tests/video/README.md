# Video Fixtures

Drop user-supplied MP4 fixtures into this directory to exercise end-to-end motion detection.

## Filename rules

- `name-12.mp4`: motion should first be detected at or after second `12`
- `name-none.mp4`: no motion should be detected

## Notes

- Keep fixtures short and low resolution so the test suite stays fast.
- The fixture test samples frames with the standard motion-detection settings, but reads file input as fast as possible instead of real time.
- The test allows up to `+1` second after the expected start time, and it fails if motion is detected earlier than the filename says.
