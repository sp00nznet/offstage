//! `offstage install-driver <inf>`: what `devcon install` does, so neither the source build nor
//! the release needs devcon.exe (a WDK tool we can't redistribute). Creates the root-enumerated
//! device node if there isn't one, then installs the INF's driver on it. Needs admin.
//! Called by driver\install-driver.cmd after it has trusted the signing cert.

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DICD_GENERATE_ID, DIF_REGISTERDEVICE, INSTALLFLAG_FORCE, SP_DEVINFO_DATA, SPDRP_HARDWAREID,
    SetupDiCallClassInstaller, SetupDiCreateDeviceInfoList, SetupDiCreateDeviceInfoW, SetupDiDestroyDeviceInfoList,
    SetupDiGetINFClassW, SetupDiSetDeviceRegistryPropertyW, UpdateDriverForPlugAndPlayDevicesW,
};
use windows::Win32::Foundation::{ERROR_NO_SUCH_DEVINST, HWND};
use windows::core::{GUID, HSTRING, PCWSTR};

const HWID: &str = "Root\\SudoMaker\\SudoVDA";

pub fn install_driver(inf: &str) -> Result<u8, String> {
    let inf = std::path::absolute(inf).map_err(|e| format!("{inf}: {e}"))?;
    let inf = HSTRING::from(inf.as_os_str());
    let hwid = HSTRING::from(HWID);
    // An existing node (reinstall after a rebuild) only needs the driver update.
    match update(&inf, &hwid) {
        Err(e) if e.code() == ERROR_NO_SUCH_DEVINST.to_hresult() => {
            create_node(&inf)?;
            update(&inf, &hwid).map_err(|e| format!("installing the driver: {e}"))?;
        }
        r => r.map_err(|e| format!("updating the driver: {e}"))?,
    }
    eprintln!("offstage: driver installed");
    Ok(0)
}

fn update(inf: &HSTRING, hwid: &HSTRING) -> windows::core::Result<()> {
    unsafe { UpdateDriverForPlugAndPlayDevicesW(None::<HWND>, hwid, inf, INSTALLFLAG_FORCE, None) }
}

fn create_node(inf: &HSTRING) -> Result<(), String> {
    unsafe {
        let mut class = GUID::zeroed();
        let mut class_name = [0u16; 32];
        SetupDiGetINFClassW(inf, &mut class, &mut class_name, None).map_err(|e| format!("reading the INF: {e}"))?;
        let set = SetupDiCreateDeviceInfoList(Some(&class), None).map_err(|e| e.to_string())?;
        let result = (|| {
            let mut dev = SP_DEVINFO_DATA {
                cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            SetupDiCreateDeviceInfoW(
                set,
                PCWSTR(class_name.as_ptr()),
                &class,
                None,
                None,
                DICD_GENERATE_ID,
                Some(&mut dev),
            )
            .map_err(|e| format!("creating the device node: {e}"))?;
            // Hardware IDs are a REG_MULTI_SZ: the ID, a NUL, and a closing NUL.
            let ids: Vec<u8> = HWID.encode_utf16().chain([0, 0]).flat_map(u16::to_le_bytes).collect();
            SetupDiSetDeviceRegistryPropertyW(set, &mut dev, SPDRP_HARDWAREID, Some(&ids))
                .map_err(|e| format!("setting the hardware ID: {e}"))?;
            SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set, Some(&dev))
                .map_err(|e| format!("registering the device node: {e}"))
        })();
        let _ = SetupDiDestroyDeviceInfoList(set);
        result
    }
}
