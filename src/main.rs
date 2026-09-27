//! offstage: run a program on a throwaway virtual monitor of any size, and record it or watch it
//! in a window. The monitor comes from the SudoVDA driver (see vda.rs); recording is ffmpeg's
//! ddagrab, and the viewer is ffplay's gdigrab. docs/architecture.md has the whole flow.

mod display;
mod install;
mod vda;

use display::{GdiName, name_str};
use std::collections::HashMap;
use std::io::Write;
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, RECT, TRUE};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
    SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
};
use windows::core::{BOOL, GUID};

const USAGE: &str = "usage:
  offstage run [--size WxH[@HZ]] [--record FILE] [--encoder ENC] [--view] [--wait] -- PROGRAM [ARGS...]
      Add a virtual monitor (default 1920x1080@60), run PROGRAM with its windows kept on it,
      and remove the monitor when PROGRAM exits. --record captures the monitor with ffmpeg
      (default encoder h264_nvenc); --view shows it live in a normal window.
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

/// One live virtual monitor. Dropping it unplugs it.
struct Monitor<'a> {
    vda: &'a vda::Vda,
    guid: GUID,
    name: GdiName,
    rect: RECT,
    dxgi: Option<(u32, u32)>,
    mode: (u32, u32, u32),
}

impl<'a> Monitor<'a> {
    fn add(vda: &'a vda::Vda, (w, h, hz): (u32, u32, u32)) -> Result<Self, String> {
        let guid = GUID::new().map_err(|e| e.to_string())?;
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

    fn record(&self, file: &str, encoder: &str) -> Result<Child, String> {
        let (a, o) = self
            .dxgi
            .ok_or("the monitor has no DXGI output, so ddagrab can't capture it")?;
        let hz = self.mode.2;
        // Hardware encoders take ddagrab's D3D11 frames directly; software ones need them in RAM.
        let hw = ["_nvenc", "_amf", "_qsv"].iter().any(|s| encoder.ends_with(s));
        let graph = format!(
            "ddagrab=output_idx={o}:framerate={hz}{}",
            if hw { "" } else { ",hwdownload,format=bgra" }
        );
        let mut c = Command::new("ffmpeg");
        c.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-init_hw_device",
            &format!("d3d11va=dd:{a}"),
            "-filter_hw_device",
            "dd",
        ])
        .args(["-filter_complex", &graph, "-c:v", encoder]);
        if !hw {
            c.args(["-pix_fmt", "yuv420p"]);
        }
        spawn(c.args(["-y", file]).stdin(Stdio::piped()), "ffmpeg")
    }
}

impl Drop for Monitor<'_> {
    fn drop(&mut self) {
        self.vda.remove(self.guid);
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
    let mon = Monitor::add(&vda, mode)?;
    eprintln!("offstage: {}", mon.describe());
    let viewer = if view { Some(mon.view()?) } else { None };
    let recorder = record.as_deref().map(|f| mon.record(f, &encoder)).transpose()?;

    // The program goes in a job, so its windows can be told apart from everyone else's (including
    // grandchildren a launcher spawns), and the whole tree dies with offstage. offstage joins the
    // job itself only after ffmpeg/ffplay are started, so those stay out of it.
    let job = Job::new()?;
    job.assign_self()?;
    let mut child = spawn(Command::new(exe).args(exe_args), exe)?;

    let mut in_job = HashMap::new();
    let code = loop {
        vda.ping();
        job.sweep_windows(mon.rect, &mut in_job);
        if let Ok(Some(status)) = child.try_wait() {
            break status.code().unwrap_or(1) as u8;
        }
        if STOP.load(Ordering::SeqCst) {
            break 130;
        }
        sleep(TICK);
    };

    if let Some(rec) = recorder {
        stop_recorder(rec);
        eprintln!("offstage: recorded {}", record.unwrap_or_default());
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
        let mon = Monitor::add(&vda, m)?;
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
