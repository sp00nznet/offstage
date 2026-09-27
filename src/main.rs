//! offstage: run a program on a throwaway virtual monitor of any size, and record it or watch it
//! in a window. The monitor comes from the SudoVDA driver (see vda.rs); recording is ffmpeg's
//! ddagrab, and the viewer is ffplay's gdigrab. docs/architecture.md has the whole flow.

mod display;
mod install;
mod vda;

use display::{GdiName, name_str};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, RECT, TRUE, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows::Win32::System::Console::{FreeConsole, SetConsoleCtrlHandler};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId};
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetCurrentProcessId, OpenMutexW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    ReleaseMutex, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject,
};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
    SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
};
use windows::core::{BOOL, GUID, HSTRING};

const USAGE: &str = "usage:
  offstage run [--size WxH[@HZ]] [--record FILE] [--encoder ENC] [--view] [--wait] -- PROGRAM [ARGS...]
      Run PROGRAM on a virtual monitor (default 1920x1080@60) with its windows kept there.
      --record captures the monitor with ffmpeg (default encoder h264_nvenc); --view shows it
      live in a normal window. With `offstage serve` running, borrows and resizes a pooled monitor
      (no screen flash); otherwise adds one and removes it when PROGRAM exits.
  offstage serve [--slots N] [--detach]
      Keep N pooled monitors (default 2) plugged in until stopped, so runs never plug or unplug
      one. Plugging and unplugging makes Windows blank every screen; resizing doesn't. --detach
      drops the console window (for starting at logon: scripts/install-serve-autostart.cmd).
  offstage hold [--view] [--wait] WxH[@HZ] [WxH[@HZ]...]
      Add monitors and keep them until Ctrl+C.
  offstage install-driver INF
      Admin. Create the driver's device node and install INF on it; driver/install-driver.cmd
      runs this after trusting the signing cert.
  Virtual monitors only work when this session is on the console, not over RDP. --wait queues
  until it is (for example after RDP disconnects with the park task installed; docs/rdp.md).";

/// Tick for the watchdog ping and the window sweep. Short, because a new window sits on the
/// wrong monitor until the next sweep moves it.
const TICK: Duration = Duration::from_millis(100);

static STOP: AtomicBool = AtomicBool::new(false);

extern "system" fn on_ctrl(_: u32) -> BOOL {
    STOP.store(true, Ordering::SeqCst);
    TRUE
}

fn main() -> ExitCode {
    // Physical pixels everywhere. Otherwise, on a scaled monitor, window and monitor rectangles come
    // back virtualised and don't match what ffmpeg captures.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]),
        Some("hold") => hold(&args[1..]),
        Some("serve") => serve(&args[1..]),
        Some("install-driver") if args.len() == 2 => install::install_driver(&args[1]),
        _ => Err(USAGE.into()),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("offstage: {e}");
            ExitCode::from(2)
        }
    }
}

/// "2560x1440" or "2560x1440@144", within what SudoVDA accepts.
fn parse_mode(s: &str) -> Result<(u32, u32, u32), String> {
    let bad = || format!("bad size {s:?}: want WxH or WxH@HZ, 640x480 to 7680x4320");
    let (wh, hz) = s.split_once('@').unwrap_or((s, "60"));
    let (w, h) = wh.split_once(['x', 'X']).ok_or_else(bad)?;
    let (w, h, hz): (u32, u32, u32) = (
        w.parse().map_err(|_| bad())?,
        h.parse().map_err(|_| bad())?,
        hz.parse().map_err(|_| bad())?,
    );
    if !(640..=7680).contains(&w) || !(480..=4320).contains(&h) || !(1..=500).contains(&hz) {
        return Err(bad());
    }
    Ok((w, h, hz))
}

/// Pool slots `serve` and `run` share. The driver's default limit is 10 monitors.
const MAX_SLOTS: u32 = 8;
const SERVE_MUTEX: &str = r"Local\offstage-serve";

/// Fixed per slot, so every process names the same monitor: adding a GUID the driver already
/// has returns the existing monitor instead of plugging a new one.
fn slot_guid(slot: u32) -> GUID {
    GUID::from_u128(0x6f666673_7461_6765_736c_6f7400000000 + slot as u128)
}

/// A pool slot this process holds, so two runs never share a monitor. The mutex is released on
/// drop, or abandoned if offstage dies, and claim_slot accepts abandoned ones.
struct Slot(HANDLE);

impl Drop for Slot {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

/// The first free pool slot, if `serve` is running. None means use a temporary monitor.
fn claim_slot() -> Option<(u32, Slot)> {
    unsafe {
        let serve = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, &HSTRING::from(SERVE_MUTEX)).ok()?;
        let _ = CloseHandle(serve);
        for i in 0..MAX_SLOTS {
            let Ok(h) = CreateMutexW(None, false, &HSTRING::from(format!(r"Local\offstage-slot-{i}"))) else {
                continue;
            };
            let w = WaitForSingleObject(h, 0);
            if w == WAIT_OBJECT_0 || w == WAIT_ABANDONED {
                return Some((i, Slot(h)));
            }
            let _ = CloseHandle(h);
        }
        None
    }
}

/// One live virtual monitor. Dropping a temporary one unplugs it; a pooled one stays for the next run.
struct Monitor<'a> {
    vda: &'a vda::Vda,
    guid: GUID,
    pooled: bool,
    name: GdiName,
    rect: RECT,
    dxgi: Option<(u32, u32)>,
    mode: (u32, u32, u32),
}

impl<'a> Monitor<'a> {
    /// With a slot, reuses (and resizes) that pool monitor, plugging it in only if it isn't yet.
    fn add(vda: &'a vda::Vda, (w, h, hz): (u32, u32, u32), slot: Option<u32>) -> Result<Self, String> {
        let guid = match slot {
            Some(i) => slot_guid(i),
            None => GUID::new().map_err(|e| e.to_string())?,
        };
        let out = vda.add(guid, w, h, hz)?;
        // Windows takes a moment to bring the monitor up; the driver needs pings meanwhile.
        let deadline = Instant::now() + Duration::from_secs(10);
        let name = loop {
            if let Some(n) = display::gdi_name(out.adapter, out.target_id) {
                break n;
            }
            if Instant::now() > deadline {
                vda.remove(guid);
                return Err("the monitor was added but never showed up in the display configuration".into());
            }
            vda.ping();
            sleep(TICK);
        };
        let mut m = Monitor {
            vda,
            guid,
            pooled: slot.is_some(),
            name,
            rect: RECT::default(),
            dxgi: None,
            mode: (w, h, hz),
        };
        display::set_mode(&m.name, w, h, hz)?;
        // The desktop extends onto the monitor after the path exists; wait for it at full size.
        let rect = loop {
            if let Some(r) =
                display::rect(&m.name).filter(|r| (r.right - r.left, r.bottom - r.top) == (w as i32, h as i32))
            {
                break r;
            }
            if Instant::now() > deadline {
                return Err(format!("{} never joined the desktop at {w}x{h}", name_str(&m.name)));
            }
            vda.ping();
            sleep(TICK);
        };
        // Fill in `m` rather than build a new Monitor from it: `..m` would copy the fields, then
        // drop `m`, and Drop unplugs the monitor.
        m.rect = rect;
        m.dxgi = display::dxgi_index(&m.name);
        Ok(m)
    }

    fn describe(&self) -> String {
        let (w, h, hz) = self.mode;
        let r = self.rect;
        let dxgi = self
            .dxgi
            .map_or("none".into(), |(a, o)| format!("adapter {a} output {o}"));
        format!(
            "{} {w}x{h}@{hz} at ({}, {}), dxgi {dxgi}",
            name_str(&self.name),
            r.left,
            r.top
        )
    }

    /// ffplay window showing this monitor live. gdigrab takes desktop coordinates, which sidesteps
    /// ffplay having no way to pick the GPU adapter the way ddagrab needs.
    fn view(&self) -> Result<Child, String> {
        let (w, h, _) = self.mode;
        spawn(
            Command::new("ffplay")
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "gdigrab",
                    "-framerate",
                    "30",
                ])
                .args([
                    "-offset_x",
                    &self.rect.left.to_string(),
                    "-offset_y",
                    &self.rect.top.to_string(),
                ])
                .args([
                    "-video_size",
                    &format!("{w}x{h}"),
                    "-window_title",
                    &format!("offstage {}", name_str(&self.name)),
                ])
                .args(["-x", "960", "-y", &(960 * h / w).to_string(), "-i", "desktop"]),
            "ffplay",
        )
    }
}

/// Records one monitor with ffmpeg ddagrab, in segments. Any display-mode change on the GPU (another
/// run resizing its pooled monitor, a UAC prompt) ends every desktop duplication with
/// DXGI_ERROR_ACCESS_LOST (887a0026) and ffmpeg exits. So the recorder starts a new segment, and
/// joins the segments when stopped. A gap of a fraction of a second beats a truncated recording.
struct Recorder {
    file: PathBuf,
    encoder: String,
    hz: u32,
    name: GdiName,
    segments: Vec<PathBuf>,
    child: Option<Child>,
    started: Instant,
    quick_fails: u32,
}

impl Recorder {
    fn start(file: &str, encoder: &str, mon: &Monitor) -> Result<Self, String> {
        let mut r = Recorder {
            file: PathBuf::from(file),
            encoder: encoder.into(),
            hz: mon.mode.2,
            name: mon.name,
            segments: Vec::new(),
            child: None,
            started: Instant::now(),
            quick_fails: 0,
        };
        r.segment()?;
        Ok(r)
    }

    fn segment(&mut self) -> Result<(), String> {
        // Looked up per segment: plugging or resizing another monitor can renumber outputs.
        let (a, o) =
            display::dxgi_index(&self.name).ok_or("the monitor has no DXGI output, so ddagrab can't capture it")?;
        let path = self.file.with_extension(format!("part{}.mp4", self.segments.len()));
        // Hardware encoders take ddagrab's D3D11 frames directly; software ones need them in RAM.
        let hw = ["_nvenc", "_amf", "_qsv"].iter().any(|s| self.encoder.ends_with(s));
        let graph = format!(
            "ddagrab=output_idx={o}:framerate={}{}",
            self.hz,
            if hw { "" } else { ",hwdownload,format=bgra" }
        );
        let mut c = Command::new("ffmpeg");
        c.args([
            "-hide_banner",
            "-loglevel",
            "fatal",
            "-init_hw_device",
            &format!("d3d11va=dd:{a}"),
        ])
        .args([
            "-filter_hw_device",
            "dd",
            "-filter_complex",
            &graph,
            "-c:v",
            &self.encoder,
        ]);
        if !hw {
            c.args(["-pix_fmt", "yuv420p"]);
        }
        self.child = Some(spawn(c.arg("-y").arg(&path).stdin(Stdio::piped()), "ffmpeg")?);
        self.segments.push(path);
        self.started = Instant::now();
        Ok(())
    }

    /// Called every tick: if ffmpeg has dropped out, start the next segment.
    fn tick(&mut self) {
        let Some(c) = self.child.as_mut() else { return };
        if !matches!(c.try_wait(), Ok(Some(_))) {
            return;
        }
        self.child = None;
        // A segment that dies at once is ffmpeg failing, not a display change; don't spin on it.
        self.quick_fails = if self.started.elapsed() < Duration::from_secs(2) {
            self.quick_fails + 1
        } else {
            0
        };
        if self.quick_fails >= 5 {
            eprintln!("offstage: ffmpeg keeps failing at start; recording stopped");
            return;
        }
        eprintln!("offstage: capture interrupted (a display changed); continuing in a new segment");
        if let Err(e) = self.segment() {
            eprintln!("offstage: {e}; recording stopped");
        }
    }

    fn finish(mut self) -> Result<(), String> {
        if let Some(c) = self.child.take() {
            stop_recorder(c);
        }
        let parts: Vec<&PathBuf> = self
            .segments
            .iter()
            .filter(|p| p.metadata().is_ok_and(|m| m.len() > 0))
            .collect();
        let _ = std::fs::remove_file(&self.file);
        match parts[..] {
            [] => return Err("no frames were recorded".into()),
            [one] => std::fs::rename(one, &self.file).map_err(|e| e.to_string())?,
            _ => {
                let list = self.file.with_extension("parts.txt");
                let lines: String = parts
                    .iter()
                    .map(|p| format!("file '{}'\n", p.display().to_string().replace('\'', r"'\''")))
                    .collect();
                std::fs::write(&list, lines).map_err(|e| e.to_string())?;
                let ok = Command::new("ffmpeg")
                    .args(["-hide_banner", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i"])
                    .arg(&list)
                    .args(["-c", "copy", "-y"])
                    .arg(&self.file)
                    .status()
                    .is_ok_and(|s| s.success());
                if !ok {
                    return Err(format!(
                        "joining segments failed; they are kept, listed in {}",
                        list.display()
                    ));
                }
                let _ = std::fs::remove_file(&list);
                for p in &self.segments {
                    let _ = std::fs::remove_file(p);
                }
                eprintln!("offstage: joined {} segments", parts.len());
            }
        }
        Ok(())
    }
}

impl Drop for Monitor<'_> {
    fn drop(&mut self) {
        if !self.pooled {
            self.vda.remove(self.guid);
        }
    }
}

fn spawn(c: &mut Command, what: &str) -> Result<Child, String> {
    c.spawn().map_err(|e| format!("starting {what} (is it on PATH?): {e}"))
}

/// ffmpeg finalises the mp4 on "q"; killing it leaves a file with no index.
fn stop_recorder(mut rec: Child) {
    if let Some(stdin) = rec.stdin.as_mut() {
        let _ = stdin.write_all(b"q");
    }
    drop(rec.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = rec.try_wait() {
            return;
        }
        sleep(TICK);
    }
    eprintln!("offstage: ffmpeg didn't finish in 15s; the recording may be truncated");
    let _ = rec.kill();
}

/// True when this process's session is the one on the console. Virtual monitors attach to the
/// console session only; over RDP, or while disconnected, they never reach this session.
fn at_console() -> bool {
    let mut mine = 0;
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut mine).is_ok() && mine == WTSGetActiveConsoleSessionId() }
}

/// Fails fast off the console, or with `wait`, blocks until the session is back on it.
fn console_gate(wait: bool) -> Result<(), String> {
    if at_console() {
        return Ok(());
    }
    if !wait {
        return Err("this session isn't on the console (RDP, or disconnected), so a virtual monitor can't reach it. Use --wait to queue, or connect with VNC/Parsec/Moonlight; see docs/rdp.md".into());
    }
    eprintln!("offstage: waiting for this session to return to the console (see docs/rdp.md)");
    while !at_console() {
        if STOP.load(Ordering::SeqCst) {
            return Err("cancelled while waiting for the console".into());
        }
        sleep(Duration::from_secs(2));
    }
    // ponytail: a fixed settle; the console's displays come up a moment after the session lands.
    sleep(Duration::from_secs(5));
    Ok(())
}

fn run(args: &[String]) -> Result<u8, String> {
    let (mut mode, mut record, mut encoder, mut view) = ((1920, 1080, 60), None, "h264_nvenc".to_string(), false);
    let mut wait = false;
    let mut it = args.iter();
    let program: Vec<&String> = loop {
        match it.next().map(String::as_str) {
            Some("--size") => mode = parse_mode(it.next().ok_or(USAGE)?)?,
            Some("--record") => record = Some(it.next().ok_or(USAGE)?.clone()),
            Some("--encoder") => encoder = it.next().ok_or(USAGE)?.clone(),
            Some("--view") => view = true,
            Some("--wait") => wait = true,
            Some("--") => break it.collect(),
            _ => return Err(USAGE.into()),
        }
    };
    let (exe, exe_args) = program.split_first().ok_or(USAGE)?;

    unsafe { SetConsoleCtrlHandler(Some(on_ctrl), true).map_err(|e| e.to_string())? };
    console_gate(wait)?;
    let vda = vda::Vda::open()?;
    // Held to the end of run, so no other run takes this slot's monitor meanwhile.
    let slot = claim_slot();
    let mon = Monitor::add(&vda, mode, slot.as_ref().map(|(i, _)| *i))?;
    match &slot {
        Some((i, _)) => eprintln!("offstage: {} (pool slot {i})", mon.describe()),
        None => eprintln!(
            "offstage: {} (temporary; run `offstage serve` to avoid the screen flash)",
            mon.describe()
        ),
    }
    let viewer = if view { Some(mon.view()?) } else { None };
    let mut recorder = record
        .as_deref()
        .map(|f| Recorder::start(f, &encoder, &mon))
        .transpose()?;

    // The program goes in a job, so its windows can be told apart from everyone else's (including
    // grandchildren a launcher spawns), and the whole tree dies with offstage. offstage joins the
    // job itself only after ffmpeg/ffplay are started, so those stay out of it.
    let job = Job::new()?;
    job.assign_self()?;
    let mut child = spawn(Command::new(exe).args(exe_args), exe)?;

    let mut in_job = HashMap::new();
    let mut rect = mon.rect;
    let code = loop {
        vda.ping();
        // Resizing a pooled monitor to the left of this one moves this one.
        rect = display::rect(&mon.name).unwrap_or(rect);
        job.sweep_windows(rect, &mut in_job);
        if let Some(r) = recorder.as_mut() {
            r.tick();
        }
        if let Ok(Some(status)) = child.try_wait() {
            break status.code().unwrap_or(1) as u8;
        }
        if STOP.load(Ordering::SeqCst) {
            break 130;
        }
        sleep(TICK);
    };

    if let Some(rec) = recorder {
        match rec.finish() {
            Ok(()) => eprintln!("offstage: recorded {}", record.unwrap_or_default()),
            Err(e) => eprintln!("offstage: recording: {e}"),
        }
    }
    if let Some(mut v) = viewer {
        let _ = v.kill();
    }
    Ok(code)
}

fn hold(args: &[String]) -> Result<u8, String> {
    let view = args.iter().any(|a| a == "--view");
    let wait = args.iter().any(|a| a == "--wait");
    let modes = args
        .iter()
        .filter(|a| *a != "--view" && *a != "--wait")
        .map(|a| parse_mode(a))
        .collect::<Result<Vec<_>, _>>()?;
    if modes.is_empty() {
        return Err(USAGE.into());
    }
    unsafe { SetConsoleCtrlHandler(Some(on_ctrl), true).map_err(|e| e.to_string())? };
    console_gate(wait)?;
    let vda = vda::Vda::open()?;
    let mut mons = Vec::new();
    for m in modes {
        let mon = Monitor::add(&vda, m, None)?;
        eprintln!("offstage: {}", mon.describe());
        mons.push(mon);
    }
    let mut viewers = Vec::new();
    if view {
        for m in &mons {
            viewers.push(m.view()?);
        }
    }
    eprintln!("offstage: holding {} monitor(s); Ctrl+C to remove", mons.len());
    while !STOP.load(Ordering::SeqCst) {
        vda.ping();
        sleep(Duration::from_secs(1));
    }
    for mut v in viewers {
        let _ = v.kill();
    }
    Ok(0)
}

fn serve(args: &[String]) -> Result<u8, String> {
    let (mut slots, mut detach) = (2, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--slots" => {
                slots = it
                    .next()
                    .and_then(|n| n.parse().ok())
                    .filter(|n| (1..=MAX_SLOTS).contains(n))
                    .ok_or(format!("--slots wants 1 to {MAX_SLOTS}"))?
            }
            "--detach" => detach = true,
            _ => return Err(USAGE.into()),
        }
    }
    unsafe { SetConsoleCtrlHandler(Some(on_ctrl), true).map_err(|e| e.to_string())? };
    // Held for serve's lifetime; claim_slot looks for it to know the pool exists.
    let keeper = unsafe { CreateMutexW(None, true, &HSTRING::from(SERVE_MUTEX)) }.map_err(|e| e.to_string())?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return Err("serve is already running".into());
    }
    // Started at logon, the session may be RDP; the pool can only live on the console.
    console_gate(true)?;
    let vda = vda::Vda::open()?;
    for i in 0..slots {
        let mon = Monitor::add(&vda, (1920, 1080, 60), Some(i))?;
        eprintln!("offstage: pool slot {i}: {}", mon.describe());
    }
    eprintln!("offstage: serving {slots} pooled monitor(s); Ctrl+C to stop");
    if detach {
        unsafe {
            let _ = FreeConsole();
        }
    }
    while !STOP.load(Ordering::SeqCst) {
        vda.ping();
        sleep(Duration::from_secs(1));
    }
    // Runs may have grown the pool past `slots`, so clear every slot.
    for i in 0..MAX_SLOTS {
        vda.remove(slot_guid(i));
    }
    unsafe {
        let _ = CloseHandle(keeper);
    }
    Ok(0)
}

struct Job(HANDLE);

impl Job {
    fn new() -> Result<Self, String> {
        unsafe {
            let h = CreateJobObjectW(None, None).map_err(|e| e.to_string())?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as _,
                size_of_val(&info) as u32,
            )
            .map_err(|e| e.to_string())?;
            Ok(Job(h))
        }
    }

    fn assign_self(&self) -> Result<(), String> {
        unsafe { AssignProcessToJobObject(self.0, GetCurrentProcess()).map_err(|e| format!("joining job: {e}")) }
    }

    /// Moves every visible top-level window owned by a process in the job onto `target`, unless
    /// its centre is already there.
    // ponytail: moves, never resizes. A game that sized itself to the primary monitor before the
    // first sweep keeps that size; fix is launching with the virtual monitor as primary.
    fn sweep_windows(&self, target: RECT, cache: &mut HashMap<u32, bool>) {
        struct Ctx<'a> {
            job: HANDLE,
            target: RECT,
            cache: &'a mut HashMap<u32, bool>,
        }
        extern "system" fn each(hwnd: HWND, lp: LPARAM) -> BOOL {
            let ctx = unsafe { &mut *(lp.0 as *mut Ctx) };
            unsafe {
                if !IsWindowVisible(hwnd).as_bool() {
                    return TRUE;
                }
                let mut pid = 0;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                let ours = *ctx.cache.entry(pid).or_insert_with(|| {
                    let Ok(p) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                        return false;
                    };
                    let mut b = BOOL(0);
                    let _ = IsProcessInJob(p, Some(ctx.job), &mut b);
                    let _ = CloseHandle(p);
                    b.as_bool()
                });
                let mut r = RECT::default();
                if !ours || GetWindowRect(hwnd, &mut r).is_err() {
                    return TRUE;
                }
                let t = ctx.target;
                let (cx, cy) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
                if cx < t.left || cx >= t.right || cy < t.top || cy >= t.bottom {
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        t.left,
                        t.top,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
                    );
                }
            }
            TRUE
        }
        let mut ctx = Ctx {
            job: self.0,
            target,
            cache,
        };
        unsafe {
            let _ = EnumWindows(Some(each), LPARAM(&mut ctx as *mut Ctx as isize));
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_mode;

    #[test]
    fn modes() {
        assert_eq!(parse_mode("2560x1440"), Ok((2560, 1440, 60)));
        assert_eq!(parse_mode("3840X2160@144"), Ok((3840, 2160, 144)));
        for bad in [
            "",
            "1920",
            "1920x",
            "x1080",
            "100x100",
            "8000x4320",
            "1920x1080@0",
            "1920x1080@",
            "axb",
        ] {
            assert!(parse_mode(bad).is_err(), "{bad}");
        }
    }
}
