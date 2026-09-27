# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: SemVer.

## [Unreleased]

### Added
- `offstage run`: runs a program on a new virtual monitor of any size (640x480 to 8K), keeps its
  process tree's windows there, and can record it (`--record`, ffmpeg ddagrab) or show it live (`--view`).
- `offstage hold`: several monitors at once until Ctrl+C, each with an optional viewer window.
- `driver/build-driver.cmd`: builds and signs SudoVDA from source with the WDK NuGet packages, no admin.
- `driver/install-driver.cmd`: installs or uninstalls it (admin).
- `--wait` on `run` and `hold`: off the console (RDP, disconnected), queue until the session is back on it.
- `scripts/install-park-task.cmd`: on RDP disconnect, move the session back to the console (admin, opt-in).

### Fixed
- The monitor was unplugged as soon as it was added (a temporary `Monitor` ran its `Drop`).
- Off the console, offstage now fails at once rather than after a 10 s wait.
