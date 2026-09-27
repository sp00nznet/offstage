# RDP

**offstage does not work from inside an RDP session.** Use VNC, Parsec, or Moonlight to reach the
machine instead: they show the console session, which is where virtual monitors live.

## What happens

RDP moves your whole Windows session onto the RDP display adapter. The console is left at the
logon screen (`query session` shows `console ... Conn` with no user). SudoVDA's monitors always
attach to the console session. From inside RDP, the monitor is created but never appears in your
session's display configuration, and offstage gives up after 10 s (Windows 11 Pro 26200):

```
PS> offstage hold --view 1920x1080
offstage: the monitor was added but never showed up in this RDP session; it has likely attached to the console session instead. See docs/rdp.md
```

offstage now checks the session up front (`ProcessIdToSessionId` against
`WTSGetActiveConsoleSessionId`) and fails at once, without touching the driver:

```
$ offstage hold 1920x1080
offstage: this session isn't on the console (RDP, or disconnected), so a virtual monitor can't reach it. Use --wait to queue, or connect with VNC/Parsec/Moonlight; see docs/rdp.md
```

There is no way to add a monitor to the RDP session itself. The client sends its monitor layout
when it connects, and the phone app always sends one screen.

Client Windows allows one interactive session. So there is no way to have your RDP session and a
console session with virtual monitors at the same time.

## What works

- **VNC, Parsec, Moonlight/Sunshine.** These stream the console session, so offstage works exactly as
  it does at the machine. Don't use Apollo: it installs its own SudoVDA, which shares a hardware ID
  with ours (see [driver.md](driver.md)).
- **Queue from RDP, run after you disconnect.** `offstage run --wait ...` (or `hold --wait`) checks
  where the session is. Off the console, it waits until the session is back on it, then runs:

  ```
  $ offstage run --wait -- cmd /c echo hi
  offstage: waiting for this session to return to the console (see docs/rdp.md)
  ```

  Just closing RDP leaves the session *disconnected*, not on the console, so nothing would start.
  `scripts\install-park-task.cmd` (admin, once) fixes that. It installs a task that runs
  `scripts\park.ps1` as SYSTEM on every RDP disconnect, which runs `tscon <id> /dest:console`.
  Reconnecting over RDP takes the session back as usual. Removal: `install-park-task.cmd uninstall`.
  The parse in park.ps1 was checked against real `query session` output; the task itself has not
  yet run through a live disconnect.

  **Security:** while parked, the session is unlocked on the machine's own screens. Anyone
  physically at the machine has your desktop. Locking it would stop capture, since the lock screen
  replaces the desktop that ffmpeg duplicates. Only install the task on a machine where that's
  acceptable.
- **Capture itself works under RDP.** Both ffmpeg capture paths record the RDP display:

  ```
  ffmpeg -init_hw_device d3d11va=dd:0 -filter_hw_device dd -filter_complex "ddagrab=output_idx=0:framerate=30" -c:v h264_nvenc -t 2 -y rdp_test.mp4   -> exit 0, 532 KB
  ffmpeg -f gdigrab -framerate 30 -t 2 -i desktop -y rdp_gdi.mp4                                                                                     -> exit 0, 818 KB
  ```
