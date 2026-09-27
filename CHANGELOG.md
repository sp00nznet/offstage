# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: SemVer.

## [Unreleased]

### Fixed
- `run` always exited 0, whatever the program returned. Closing the kill-on-close job, which offstage
  is in too, killed offstage before its exit code was set. It now exits with the program's code.
- A lost or incomplete recording now fails the run with exit code 3 when the program itself
  succeeded, instead of reporting success.
- Restarting capture after a display change waits 500 ms between attempts and gives up after 20
  quick failures. It used to give up after 5, which a burst of display changes could cause.
- A pooled monitor now takes sizes outside SudoVDA's built-in list (e.g. 1600x900, 1440x900). The
  slot is replugged at that size once, with one screen flash.

### Changed
- `scripts/install-serve-autostart.cmd` runs serve from a copy in `%LOCALAPPDATA%\offstage`, so a
  running serve no longer blocks `cargo build --release`.

## [0.2.0] - 2026-09-27

### Added
- `offstage serve`: keeps a pool of virtual monitors plugged in. `run` borrows and resizes one
  instead of plugging its own, so Windows no longer blanks every screen on each run.
- `scripts/install-serve-autostart.cmd`: starts `serve --detach` at logon (no admin).

### Fixed
- A recording no longer stops when another display changes mode, such as a concurrent run resizing
  its monitor. It continues in a new segment, and the segments are joined at the end.

## [0.1.0] - 2026-09-27

### Added
- `offstage run`: runs a program on a new virtual monitor of any size (640x480 to 8K), keeps its
  process tree's windows there, and can record it (`--record`, ffmpeg ddagrab) or show it live (`--view`).
- `offstage hold`: several monitors at once until Ctrl+C, each with an optional viewer window.
- `--wait` on `run` and `hold`: off the console (RDP, disconnected), queue until the session is back on it.
- `offstage install-driver`: creates the driver's device node and installs it (admin), so no devcon.exe is needed.
- `driver/build-driver.cmd`: builds and signs SudoVDA from source with the WDK NuGet packages, no admin.
- `driver/install-driver.cmd`: trusts the signing cert and installs the driver, or uninstalls both (admin).
- `scripts/install-park-task.cmd`: on RDP disconnect, move the session back to the console (admin, opt-in).
- Release zip built by CI, with the driver signed by a per-release key that is discarded after signing.
