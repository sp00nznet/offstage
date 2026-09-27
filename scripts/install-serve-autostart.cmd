@echo off
rem Starts `offstage serve --detach` at logon for this user (HKCU Run key, no admin). The monitor pool
rem is then plugged in once per logon, and runs borrow from it instead of flashing every screen.
rem With "uninstall", removes the entry. It doesn't stop a serve that's already running.
setlocal
set "KEY=HKCU\Software\Microsoft\Windows\CurrentVersion\Run"
if /i "%1"=="uninstall" (
  reg delete "%KEY%" /v offstage /f
  exit /b
)
set "EXE=%~dp0..\offstage.exe"
if not exist "%EXE%" set "EXE=%~dp0..\target\release\offstage.exe"
if not exist "%EXE%" (echo offstage.exe not found. From source, run cargo build --release first. & exit /b 1)
for %%i in ("%EXE%") do set "EXE=%%~fi"
reg add "%KEY%" /v offstage /t REG_SZ /d "\"%EXE%\" serve --detach" /f >nul || exit /b 1
echo Installed: starts at next logon. To start it now:  start "" "%EXE%" serve --detach
