# Run as SYSTEM by the offstage-park task when an RDP session disconnects (install-park-task.cmd).
# Moves the disconnected session back to the console, so virtual monitors reach it again and any
# `offstage run --wait` queued from the phone starts. Reconnecting over RDP takes the session back.
# Security: this leaves the console unlocked while nobody is at it. See docs/rdp.md.
Start-Sleep -Seconds 2
foreach ($line in (query session)) {
    # A disconnected user session has no session name: "   user   1  Disc". Session 0 is services.
    if ($line -match '\s(\d+)\s+Disc\s*$' -and $Matches[1] -ne '0') {
        tscon $Matches[1] /dest:console
    }
}
