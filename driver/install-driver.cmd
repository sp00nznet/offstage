@echo off
rem Installs (or with "uninstall", removes) the driver built by build-driver.cmd. Run as admin.
rem Trusts offstage.cer, the public half of this machine's own signing cert, in the machine's Root
rem and TrustedPublisher stores. That is what lets Windows load a driver that isn't Microsoft-signed
rem without test-signing mode. See docs/driver.md.
setlocal
set "OUT=%~dp0out"
set "DEVCON=%~dp0packages\microsoft.windows.wdk.x64\c\tools\10.0.28000.0\x64\devcon.exe"
set "HWID=Root\SudoMaker\SudoVDA"

net session >nul 2>&1 || (echo Run this from an elevated prompt. & exit /b 1)

if /i "%1"=="uninstall" (
  "%DEVCON%" remove "%HWID%"
  rem Deletes every sudovda.inf package, Apollo's included: they share a hardware ID and can't coexist.
  powershell -NoProfile -Command ^
    "Get-WindowsDriver -Online | ? OriginalFileName -like '*sudovda.inf' | %% { pnputil /delete-driver $_.Driver /uninstall /force };" ^
    "$t = (Get-PfxCertificate '%OUT%\offstage.cer').Thumbprint;" ^
    "'Root','TrustedPublisher' | %% { Remove-Item \"Cert:\LocalMachine\$_\$t\" -ErrorAction SilentlyContinue }"
  echo Removed.
  exit /b 0
)

if not exist "%OUT%\sudovda.cat" (echo Run build-driver.cmd first. & exit /b 1)
certutil -addstore -f Root "%OUT%\offstage.cer" >nul || exit /b 1
certutil -addstore -f TrustedPublisher "%OUT%\offstage.cer" >nul || exit /b 1

rem "update" if the device node exists (reinstall after a rebuild), "install" creates it.
"%DEVCON%" find "%HWID%" | findstr /i /c:"SudoVDA" >nul
if errorlevel 1 (
  "%DEVCON%" install "%OUT%\SudoVDA.inf" "%HWID%" || exit /b 1
) else (
  "%DEVCON%" update "%OUT%\SudoVDA.inf" "%HWID%" || exit /b 1
)
echo Installed. Try: offstage hold 1920x1080
