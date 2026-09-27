# Security

offstage changes machine-wide trust settings during setup. Know what each step does before
running it.

| Step | Runs as | What it changes |
|---|---|---|
| `driver\build-driver.cmd` | you | Creates a code-signing cert, `CN=offstage local driver signing`, in your user store, with a non-exportable private key. |
| `driver\install-driver.cmd` | admin | Adds that cert's public half to `LocalMachine\Root` and `TrustedPublisher`, and installs a user-mode display driver. |
| `scripts\install-park-task.cmd` | admin | Adds a scheduled task that runs as SYSTEM on every RDP disconnect and moves the session to the console. |
| `offstage run` / `hold` | you | Adds and removes virtual monitors, and runs ffmpeg. No elevation. |

The consequences:

- **The trusted cert.** Anything signed with your local key is trusted on this machine. The key
  can't be exported, but any code running as your user can sign with it. Remove it from
  `Cert:\CurrentUser\My` after installing if that matters to you. Details: [docs/driver.md](docs/driver.md).
- **The park task.** It leaves your session unlocked on the machine's own screens after every RDP
  disconnect. Details: [docs/rdp.md](docs/rdp.md).
- Both are undone by running the same script with `uninstall`.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting (the Security tab, "Report a vulnerability"), not a
public issue.
