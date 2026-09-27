@echo off
rem Installs (or with "uninstall", removes) the offstage-park scheduled task. Run as admin.
rem On every RDP disconnect (LocalSessionManager event 24) it runs park.ps1 as SYSTEM, which moves
rem the session back to the console. tscon to the console needs SYSTEM; that's why this is a task.
rem Security: the console is then unlocked while you're away. See docs/rdp.md.
net session >nul 2>&1 || (echo Run this from an elevated prompt. & exit /b 1)
if /i "%1"=="uninstall" (
  schtasks /delete /tn offstage-park /f
  exit /b
)
schtasks /create /f /tn offstage-park /ru SYSTEM /rl HIGHEST /sc onevent ^
  /ec Microsoft-Windows-TerminalServices-LocalSessionManager/Operational /mo "*[System[EventID=24]]" ^
  /tr "powershell -NoProfile -ExecutionPolicy Bypass -File \"%~dp0park.ps1\""
