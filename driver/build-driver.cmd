@echo off
rem Builds and signs the SudoVDA virtual display driver from the submodule. No admin needed.
rem Why cl/link and not MSBuild: the WDK NuGet packages ship headers, libs and tools, but the
rem WindowsUserModeDriver10.0 MSBuild toolset only registers when the WDK component is installed
rem into Visual Studio (admin). SudoVDA is one .cpp, so compiling it directly is simpler.
rem See docs/driver.md.
setlocal
set "HERE=%~dp0"
set "SRC=%HERE%SudoVDA\Virtual Display Driver (HDR)\SudoVDA"
set "INC=%HERE%SudoVDA\Common\Include"
set "PKG=%HERE%packages"
set "OUT=%HERE%out"
set "V=10.0.28000.2526"
set "K=10.0.28000.0"
rem CI names each release's throwaway key; locally this stays one reusable cert.
if not defined OFFSTAGE_SIGNER set "OFFSTAGE_SIGNER=offstage local driver signing"

if not exist "%SRC%\Driver.cpp" git -C "%HERE%.." submodule update --init || exit /b 1

for %%p in (microsoft.windows.wdk.x64 microsoft.windows.sdk.cpp microsoft.windows.sdk.cpp.x64) do (
  if not exist "%PKG%\%%p" (
    echo Fetching %%p %V%...
    mkdir "%PKG%\%%p" 2>nul
    curl -sfLo "%PKG%\%%p.zip" https://api.nuget.org/v3-flatcontainer/%%p/%V%/%%p.%V%.nupkg || exit /b 1
    tar -xf "%PKG%\%%p.zip" -C "%PKG%\%%p" || exit /b 1
    del "%PKG%\%%p.zip"
  )
)

for /f "usebackq delims=" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -property installationPath`) do set "VS=%%i"
call "%VS%\VC\Auxiliary\Build\vcvars64.bat" >nul || exit /b 1

rem Replace the installed SDK's paths wholesale: mixing its headers with the WDK's is how you get
rem mismatched-version errors.
set "SDKI=%PKG%\microsoft.windows.sdk.cpp\c\Include\%K%"
set "WDKI=%PKG%\microsoft.windows.wdk.x64\c\Include"
set "INCLUDE=%VCToolsInstallDir%include;%SDKI%\ucrt;%SDKI%\um;%SDKI%\shared;%SDKI%\winrt;%WDKI%\%K%\um;%WDKI%\%K%\shared;%WDKI%\wdf\umdf\2.25;%WDKI%\%K%\um\iddcx\1.10;%INC%"
set "WDKL=%PKG%\microsoft.windows.wdk.x64\c\Lib"
set "LIB=%VCToolsInstallDir%lib\x64;%PKG%\microsoft.windows.sdk.cpp.x64\c\ucrt\x64;%PKG%\microsoft.windows.sdk.cpp.x64\c\um\x64;%WDKL%\wdf\umdf\x64\2.25;%WDKL%\%K%\um\x64\iddcx\1.10"

mkdir "%OUT%" 2>nul
pushd "%OUT%"
cl /nologo /c /O2 /MT /EHsc /std:c++17 /W3 /wd4005 /DUNICODE /D_UNICODE /D_WINDOWS /D_USRDLL ^
  /DUMDF_VERSION_MAJOR=2 /DUMDF_VERSION_MINOR=25 /DUMDF_MINIMUM_VERSION_REQUIRED=25 ^
  /DIDDCX_VERSION_MAJOR=1 /DIDDCX_VERSION_MINOR=10 /DIDDCX_MINIMUM_VERSION_REQUIRED=4 ^
  "%SRC%\Driver.cpp" || (popd & exit /b 1)
link /nologo /DLL /OUT:SudoVDA.dll Driver.obj WdfDriverStubUm.lib IddCxStub.lib OneCoreUAP.lib avrt.lib ^
  d3d11.lib dxgi.lib ntdll.lib || (popd & exit /b 1)
copy /y "%SRC%\SudoVDA.inf" . >nul
rem The source INF carries $ARCH$/$UMDFVERSION$ placeholders the MSBuild toolset would fill in.
"%PKG%\microsoft.windows.wdk.x64\c\bin\%K%\x64\stampinf.exe" -f SudoVDA.inf -a amd64 -d * -v * -u 2.25.0 -k 1.15 >nul || (popd & exit /b 1)
rem Keep only package files in out\: Inf2Cat catalogs whatever is in the directory.
del Driver.obj SudoVDA.lib SudoVDA.exp
popd

rem Sign with a local cert. The private key stays non-exportable in this user's store;
rem install-driver.cmd trusts only the public half (offstage.cer).
powershell -NoProfile -Command ^
  "$c = Get-ChildItem Cert:\CurrentUser\My | ? Subject -eq 'CN=%OFFSTAGE_SIGNER%' | select -First 1;" ^
  "if (-not $c) { $c = New-SelfSignedCertificate -Subject 'CN=%OFFSTAGE_SIGNER%' -Type CodeSigningCert -CertStoreLocation Cert:\CurrentUser\My -KeyExportPolicy NonExportable -NotAfter (Get-Date).AddYears(5) };" ^
  "Export-Certificate -Cert $c -FilePath '%OUT%\offstage.cer' | Out-Null" || exit /b 1

set SIGN="%PKG%\microsoft.windows.sdk.cpp\c\bin\%K%\x64\signtool.exe" sign /q /fd sha256 /n "%OFFSTAGE_SIGNER%" /s My
rem Sign the DLL before Inf2Cat: the catalog hashes the file as signed.
%SIGN% "%OUT%\SudoVDA.dll" || exit /b 1
"%PKG%\microsoft.windows.wdk.x64\c\bin\%K%\x86\Inf2Cat.exe" /driver:"%OUT%" /os:10_X64 /uselocaltime >nul || (echo Inf2Cat failed & exit /b 1)
%SIGN% "%OUT%\sudovda.cat" || exit /b 1
echo Built and signed: %OUT%
