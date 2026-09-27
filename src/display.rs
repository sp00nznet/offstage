//! Finds the Windows view of a monitor SudoVDA just added: its GDI name (\\.\DISPLAYn), its
//! rectangle on the virtual desktop, and its DXGI (adapter, output) index for ffmpeg's ddagrab.

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS,
    QueryDisplayConfig,
};
use windows::Win32::Foundation::{LPARAM, LUID, RECT, TRUE};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows::Win32::Graphics::Gdi::{
    CDS_TYPE, ChangeDisplaySettingsExW, DEVMODEW, DISP_CHANGE_SUCCESSFUL, DM_DISPLAYFREQUENCY, DM_PELSHEIGHT,
    DM_PELSWIDTH, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows::core::{BOOL, PCWSTR};

/// GDI device name as a NUL-terminated UTF-16 buffer, the form every API here wants.
pub type GdiName = [u16; 32];

pub fn name_str(n: &GdiName) -> String {
    String::from_utf16_lossy(&n[..n.iter().position(|&c| c == 0).unwrap_or(32)])
}

/// The GDI name of the active display path whose target is (adapter, target_id), if Windows has
/// brought it up yet. None while the monitor is still arriving, or when it belongs to another
/// session (the console, while this process runs under RDP).
pub fn gdi_name(adapter: LUID, target_id: u32) -> Option<GdiName> {
    unsafe {
        let (mut np, mut nm) = (0, 0);
        GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm)
            .ok()
            .ok()?;
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut np,
            paths.as_mut_ptr(),
            &mut nm,
            modes.as_mut_ptr(),
            None,
        )
        .ok()
        .ok()?;
        let p = paths[..np as usize].iter().find(|p| {
            let t = p.targetInfo.adapterId;
            t.LowPart == adapter.LowPart && t.HighPart == adapter.HighPart && p.targetInfo.id == target_id
        })?;
        let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
        src.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
        src.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
        src.header.adapterId = p.sourceInfo.adapterId;
        src.header.id = p.sourceInfo.id;
        (DisplayConfigGetDeviceInfo(&mut src.header) == 0).then_some(src.viewGdiDeviceName)
    }
}

/// Windows picks the new monitor's first mode itself; this pins the one asked for.
pub fn set_mode(name: &GdiName, width: u32, height: u32, hz: u32) -> Result<(), String> {
    let mut dm = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    dm.dmPelsWidth = width;
    dm.dmPelsHeight = height;
    dm.dmDisplayFrequency = hz;
    dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY;
    let r = unsafe { ChangeDisplaySettingsExW(PCWSTR(name.as_ptr()), Some(&dm), None, CDS_TYPE(0), None) };
    if r == DISP_CHANGE_SUCCESSFUL {
        Ok(())
    } else {
        Err(format!(
            "setting {width}x{height}@{hz} on {}: code {}",
            name_str(name),
            r.0
        ))
    }
}

/// The monitor's rectangle in virtual-desktop coordinates, once it is part of the desktop. None
/// until then: the display path and its mode exist a moment before Windows extends the desktop
/// onto them, and gdigrab refuses a rectangle outside the desktop. Physical pixels, because
/// main() makes the process per-monitor DPI aware.
pub fn rect(name: &GdiName) -> Option<RECT> {
    struct Find<'a>(&'a GdiName, Option<RECT>);
    extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
        let f = unsafe { &mut *(lp.0 as *mut Find) };
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(m, &mut mi.monitorInfo) }.as_bool() && mi.szDevice == *f.0 {
            f.1 = Some(mi.monitorInfo.rcMonitor);
        }
        TRUE
    }
    let mut f = Find(name, None);
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut f as *mut Find as isize));
    }
    f.1
}

/// (adapter index, output index) in DXGI enumeration order. ddagrab counts outputs per adapter,
/// and its adapter is picked with `-init_hw_device d3d11va=name:<adapter index>`.
pub fn dxgi_index(name: &GdiName) -> Option<(u32, u32)> {
    unsafe {
        let f: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        for a in 0.. {
            let adapter = f.EnumAdapters1(a).ok()?;
            for o in 0.. {
                let Ok(out) = adapter.EnumOutputs(o) else { break };
                if out.GetDesc().ok()?.DeviceName == *name {
                    return Some((a, o));
                }
            }
        }
        None
    }
}
