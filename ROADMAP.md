# Roadmap

## Next
- Run the park task through a live RDP disconnect and record the result in docs/rdp.md.
- `--primary`: make the virtual monitor primary for the run, for games that size fullscreen to the
  primary monitor before offstage can move them.

## Later
- Viewer grid: one window tiling many monitors, for "8 monitors' worth" on one screen.
- VR: show virtual monitors as panels in a headset, like Virtual Desktop does.
- Driver build in CI, and a signed release of the driver package.

## Out of scope
- Headless rendering for in-house recomps. That goes in each recomp's runtime (render offscreen,
  pipe to ffmpeg), not here.
- Full isolation from the desktop (focus, input). That needs a separate machine or a GPU-partitioned VM.
