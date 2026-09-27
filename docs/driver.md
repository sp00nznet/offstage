# Building and installing the driver

## Why build it ourselves

SudoVDA publishes source but no binaries. The only prebuilt copy ships inside Apollo's installer,
signed by SudoMaker's self-signed cert (`CN=sudovda@su.mk`). Installing that means trusting
SudoMaker's key as a root on the machine. Building it means trusting a key we generated ourselves.

## Why cl/link and not MSBuild

The WDK comes as NuGet packages (`Microsoft.Windows.WDK.x64`, `Microsoft.Windows.SDK.CPP`,
`Microsoft.Windows.SDK.CPP.x64`, all pinned to `10.0.28000.2526`), so no WDK install is needed. But
SudoVDA's `.vcxproj` uses the `WindowsUserModeDriver10.0` platform toolset. That toolset only
registers when the WDK component is installed into Visual Studio, which needs admin. Without it,
MSBuild fails:

```
error MSB8020: The build tools for WindowsUserModeDriver10.0 (Platform Toolset = 'WindowsUserModeDriver10.0') cannot be found.
```

SudoVDA is one `Driver.cpp`, so `build-driver.cmd` compiles it directly. It sets the same defines
the toolset would (`UMDF_VERSION_*=2.25`, `IDDCX_VERSION_*=1.10`), and replaces `INCLUDE`/`LIB`
wholesale with the package paths so the installed SDK's headers never mix in. Things the toolset
normally does that the script does by hand:

- **`ntdll.lib`**: `WdfDriverStubUm.lib` needs `DbgPrintEx`. Without it:
  `error LNK2019: unresolved external symbol __imp_DbgPrintEx referenced in function FxDriverEntryUmWorker`.
- **`stampinf`**: the source INF has `$ARCH$` and an empty `DriverVer`. Inf2Cat rejects it as-is:
  `sudovda.inf does not have NTAMD64 decorated model sections` and
  `22.9.6: DriverVer missing or in incorrect format`.
- **The `wrl.h` include** lives under `Include\<ver>\winrt`, not `um`.
- **WPP tracing is skipped**: the project enables it, but the code never calls a trace macro.
- **Signing order**: the DLL is signed before Inf2Cat, because the catalog hashes the signed file.
- **Clean output**: only the package files stay in `out\`, since Inf2Cat catalogs whatever is in
  the directory.

## Signing and trust (security)

`build-driver.cmd` creates `CN=offstage local driver signing` in the **current user's** store, with a
**non-exportable** private key, and signs `SudoVDA.dll` and `sudovda.cat` with it.
`install-driver.cmd` adds only the public cert (`out\offstage.cer`) to `LocalMachine\Root` and
`TrustedPublisher`.

What that means: anything signed with that key is trusted on this machine, as code and as a
root. The key never leaves this user's cert store, but code running as this user can use it to
sign. If that matters on a given machine, remove the cert from `Cert:\CurrentUser\My` after
installing and rebuild with a new one next time.

`install-driver.cmd uninstall` removes the device, every `sudovda.inf` driver package (Apollo's
too, since they share a hardware ID and can't coexist), and the trusted cert.

## Settings

The driver reads `HKLM\SOFTWARE\SudoMaker\SudoVDA` (`maxMonitors` defaults to 10, `watchdog`
defaults to 3 s). offstage relies on the watchdog, so leave it non-zero.
