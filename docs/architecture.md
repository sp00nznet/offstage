# Architecture

Three parts. offstage owns none of the hard ones:

| Part | What it owns | Where |
|---|---|---|
| SudoVDA | A UMDF Indirect Display driver. Creates and removes monitors on request, of any size. | `driver/SudoVDA` (submodule), built by `driver/build-driver.cmd` |
| offstage | Asks for a monitor, finds it in Windows, keeps a program's windows on it, and runs the capture. | `src/` |
| ffmpeg / ffplay | Recording (`ddagrab`, GPU capture and encode) and the live viewer (`gdigrab`). | PATH |

## Flow of `offstage run`

1. `vda.rs` opens the driver by its device interface GUID and sends `IOCTL_ADD` with a GUID and the
   size. The driver returns the monitor's adapter LUID and target id. If `serve` is running, the
   GUID is a pool slot's fixed GUID (see [The pool](#the-pool)), and the driver hands back that
   existing monitor. Otherwise it's a fresh GUID, and a new monitor is plugged in.
2. `display.rs` polls `QueryDisplayConfig` until an active path has that (LUID, target). That gives
   the GDI name, `\\.\DISPLAYn`. offstage then pins the mode with `ChangeDisplaySettingsEx`,
   because Windows picks its own mode for a new monitor. Last, it reads the monitor's rectangle
   and finds its DXGI (adapter, output) index.
3. The recorder and viewer start **before** offstage joins its job object, so they stay out of it.
4. offstage joins a kill-on-close job, then spawns the program. Everything the program spawns
   inherits the job. That is how its windows are told apart from everyone else's, and why the
   whole tree dies when offstage does.
5. Every 100 ms: ping the driver, re-read the monitor's rectangle, move any visible job-owned
   window whose centre isn't on it, and restart the recorder if ffmpeg dropped out.
6. On exit: send ffmpeg `q` so it finalises the mp4, join the recording's segments, kill the
   viewer, and `IOCTL_REMOVE` unless the monitor is pooled.

## The pool

Plugging a monitor in or out changes the desktop layout, and Windows resets every screen: they
shrink behind black borders for a moment. On the machine this was tested on, removing one also
blacked out a 1080p monitor completely while the GPU re-established its signal. Resizing a monitor
that's already plugged in does neither. The other screens stay untouched, as watched on
2026-09-27 (4K + 1080p physical, RTX 5070).

So `offstage serve` plugs in N monitors once and pings the driver to keep them. Pool slot `i` is the
monitor with GUID `6f666673-7461-6765-736c-6f74000000ii`, the same in every process. A run claims
the first slot whose named mutex (`Local\offstage-slot-i`) it can take, resizes that monitor, and
leaves it plugged in afterwards. `serve` holds `Local\offstage-serve` for as long as it runs, and
that tells runs the pool exists. With no free slot, or no `serve`, a run falls back to a temporary
monitor and the flash that comes with it.

A SudoVDA monitor only offers the driver's fixed size list (800x600, 1280x720, 1366x768,
1920x1080, 2560x1440, 2880x1600, 3664x1920, 3840x2160, 4128x2208, 7320x3142), plus the size it
was plugged in at. So a pool monitor plugged at 1920x1080 can't be set to 1600x900. When the
resize fails, the run replugs that slot at the wanted size: one flash, and the slot keeps that
size available afterwards.

Resizing one pooled monitor moves the ones to its right, which is why the run loop re-reads its
rectangle every tick.

## Recording in segments

Any display-mode change on the GPU ends every desktop duplication with `DXGI_ERROR_ACCESS_LOST`.
Another run resizing its pooled monitor does it, and so does a UAC prompt. ffmpeg exits:

```
[Parsed_ddagrab_0 @ 000001ece8e9c940] AcquireNextFrame failed: 887a0026
```

That cut a concurrent run's recording at 27 frames. The recorder now starts a new segment (looking
up the DXGI output again, since outputs can renumber) and joins the segments with ffmpeg's concat
demuxer when the run ends. The same two-run test then gave one 9.0 s, 412-frame file with a
single join.

Restarts wait 500 ms, since a restart while the display is still changing fails again. Twenty
failures within 2 s of starting, in a row, means ffmpeg itself is failing: recording stops, and the
run exits 3 if the program succeeded.

## Exit codes and the job

offstage joins its own kill-on-close job. So it must never close the job handle itself: that
kills offstage before its exit code is set, and the run exits 0 whatever happened. Windows closes
the handle when offstage exits, which still kills whatever the program left running.

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
