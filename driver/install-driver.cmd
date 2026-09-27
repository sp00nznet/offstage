@echo off
rem Installs (or with "uninstall", removes) the SudoVDA driver in out\. Run as admin.
rem Works in the source tree (after build-driver.cmd and cargo build) and in the release zip.
rem Trusts out\offstage.cer, the public half of the cert that signed the driver, in the machine's
rem Root and TrustedPublisher stores. That is what lets Windows load a driver that isn't
rem Microsoft-signed without test-signing mode. See docs/driver.md and SECURITY.md.
setlocal
set "OUT=%~dp0out"
set "EXE=%~dp0..\offstage.exe"
if not exist "%EXE%" set "EXE=%~dp0..\target\release\offstage.exe"

net session >nul 2>&1 || (echo Run this from an elevated prompt. & exit /b 1)

if /i "%1"=="uninstall" (
  pnputil /remove-device /deviceid "Root\SudoMaker\SudoVDA"
  rem Deletes every sudovda.inf package, Apollo's included: they share a hardware ID and can't
  rem coexist. Removes every offstage signing cert, whichever build or release trusted it.
  powershell -NoProfile -Command ^
    "Get-WindowsDriver -Online | ? OriginalFileName -like '*sudovda.inf' | %% { pnputil /delete-driver $_.Driver /uninstall /force };" ^
    "'Root','TrustedPublisher' | %% { Get-ChildItem \"Cert:\LocalMachine\$_\" | ? Subject -like 'CN=offstage *' | Remove-Item }"
  echo Removed.
  exit /b 0
)

if not exist "%OUT%\sudovda.cat" (echo No driver in %OUT%. From source, run build-driver.cmd first. & exit /b 1)
if not exist "%EXE%" (echo offstage.exe not found. From source, run cargo build --release first. & exit /b 1)
certutil -addstore -f Root "%OUT%\offstage.cer" >nul || exit /b 1
certutil -addstore -f TrustedPublisher "%OUT%\offstage.cer" >nul || exit /b 1
"%EXE%" install-driver "%OUT%\SudoVDA.inf" || exit /b 1
echo Installed. Try: offstage hold 1920x1080
