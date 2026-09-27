@echo off
rem Starts `offstage serve --detach` at logon for this user (HKCU Run key, no admin). The monitor pool
rem is then plugged in once per logon, and runs borrow from it instead of flashing every screen.
rem serve runs from a copy in %LOCALAPPDATA%\offstage: a running exe is locked, and running it from
rem target\release would block every `cargo build --release` for as long as serve is up.
rem To update, stop serve, rerun this, and start it again. "uninstall" removes the entry and the copy.
setlocal
set "KEY=HKCU\Software\Microsoft\Windows\CurrentVersion\Run"
set "DEST=%LOCALAPPDATA%\offstage"
if /i "%1"=="uninstall" (
  reg delete "%KEY%" /v offstage /f
  rmdir /s /q "%DEST%" 2>nul || echo Stop serve first to remove %DEST%.
  exit /b
)
set "EXE=%~dp0..\offstage.exe"
if not exist "%EXE%" set "EXE=%~dp0..\target\release\offstage.exe"
if not exist "%EXE%" (echo offstage.exe not found. From source, run cargo build --release first. & exit /b 1)
mkdir "%DEST%" 2>nul
copy /y "%EXE%" "%DEST%\offstage.exe" >nul || (echo Copy failed: stop the running serve first. & exit /b 1)
reg add "%KEY%" /v offstage /t REG_SZ /d "\"%DEST%\offstage.exe\" serve --detach" /f >nul || exit /b 1
echo Installed: starts at next logon. To start it now:  start "" "%DEST%\offstage.exe" serve --detach
