# Architecture

Three parts. offstage owns none of the hard ones:

| Part | What it owns | Where |
|---|---|---|
| SudoVDA | A UMDF Indirect Display driver. Creates and removes monitors on request, of any size. | `driver/SudoVDA` (submodule), built by `driver/build-driver.cmd` |
| offstage | Asks for a monitor, finds it in Windows, keeps a program's windows on it, and runs the capture. | `src/` |
| ffmpeg / ffplay | Recording (`ddagrab`, GPU capture and encode) and the live viewer (`gdigrab`). | PATH |

## Flow of `offstage run`

1. `vda.rs` opens the driver by its device interface GUID and sends `IOCTL_ADD` with a fresh GUID
   and the size. The driver returns the monitor's adapter LUID and target id.
2. `display.rs` polls `QueryDisplayConfig` until an active path has that (LUID, target). That gives
   the GDI name, `\\.\DISPLAYn`. offstage then pins the mode with `ChangeDisplaySettingsEx`,
   because Windows picks its own mode for a new monitor. Last, it reads the monitor's rectangle
   and finds its DXGI (adapter, output) index.
3. The recorder and viewer start **before** offstage joins its job object, so they stay out of it.
4. offstage joins a kill-on-close job, then spawns the program. Everything the program spawns
   inherits the job. That is how its windows are told apart from everyone else's, and why the
   whole tree dies when offstage does.
5. Every 100 ms: ping the driver, and move any visible job-owned window whose centre isn't on
   the virtual monitor.
6. On exit: send ffmpeg `q` so it finalises the mp4, kill the viewer, `IOCTL_REMOVE`.

## Why these choices

- **The watchdog is a feature.** SudoVDA unplugs every monitor after 3 s without an IOCTL. So the
  100 ms loop pings, and a crashed or killed offstage never leaves a monitor behind.
- **ddagrab for recording.** It captures on the GPU and hands D3D11 frames straight to NVENC/AMF/QSV,
  which 4K at 60+ fps needs. It counts outputs *per adapter*, so offstage passes the adapter with
  `-init_hw_device d3d11va=dd:<adapter>` and the output with `output_idx`.
- **gdigrab for the viewer.** ffplay has no way to pick ddagrab's adapter. gdigrab takes plain
  virtual-desktop coordinates, and 30 fps is enough for a preview.
- **Why a job and not a PID.** Launchers spawn the real game as a grandchild. Checking
  `IsProcessInJob` catches the whole tree.
