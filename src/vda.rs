//! Client for the SudoVDA driver's IOCTL interface
//! (driver/SudoVDA/Common/Include/sudovda-ioctl.h).
//!
//! The driver's watchdog unplugs every virtual monitor after 3s with no IOCTL, so a monitor lives
//! only while `ping` keeps being called. A crashed offstage never leaves a monitor behind.

use std::mem::size_of;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CM_Get_Device_Interface_List_SizeW, CM_Get_Device_Interface_ListW, CR_SUCCESS,
};
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, LUID};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::core::{GUID, PCWSTR};

const INTERFACE: GUID = GUID::from_u128(0xe5bcc234_1e0c_418a_a0d4_ef8b7501414d);

/// CTL_CODE(FILE_DEVICE_UNKNOWN, func, METHOD_BUFFERED, FILE_ANY_ACCESS)
const fn ctl(func: u32) -> u32 {
    (0x22 << 16) | (func << 2)
}
const IOCTL_ADD: u32 = ctl(0x800);
const IOCTL_REMOVE: u32 = ctl(0x801);
const IOCTL_PING: u32 = ctl(0x888);

#[repr(C)]
struct AddParams {
    width: u32,
    height: u32,
    refresh: u32,
    guid: GUID,
    device_name: [u8; 14],
    serial: [u8; 14],
}

#[repr(C)]
#[derive(Default)]
pub struct AddOut {
    pub adapter: LUID,
    pub target_id: u32,
}

pub struct Vda(HANDLE);

impl Vda {
    pub fn open() -> Result<Self, String> {
        let not_installed = "SudoVDA driver not found. Build and install it: driver\\build-driver.cmd, then driver\\install-driver.cmd as admin";
        unsafe {
            let mut len = 0;
            if CM_Get_Device_Interface_List_SizeW(
                &mut len,
                &INTERFACE,
                PCWSTR::null(),
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            ) != CR_SUCCESS
                || len <= 1
            {
                return Err(not_installed.into());
            }
            let mut buf = vec![0u16; len as usize];
            if CM_Get_Device_Interface_ListW(
                &INTERFACE,
                PCWSTR::null(),
                &mut buf,
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            ) != CR_SUCCESS
                || buf[0] == 0
            {
                return Err(not_installed.into());
            }
            // ponytail: first interface only; one SudoVDA device is all the driver supports anyway.
            let h = CreateFileW(
                PCWSTR(buf.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
            .map_err(|e| format!("opening SudoVDA device: {e}"))?;
            Ok(Vda(h))
        }
    }

    pub fn add(&self, guid: GUID, width: u32, height: u32, hz: u32) -> Result<AddOut, String> {
        let mut name = [0u8; 14];
        name[..8].copy_from_slice(b"offstage");
        let mut serial = [0u8; 14];
        let s = format!("{:08x}", guid.data1);
        serial[..s.len()].copy_from_slice(s.as_bytes());
        let p = AddParams {
            width,
            height,
            refresh: hz,
            guid,
            device_name: name,
            serial,
        };
        let mut out = AddOut::default();
        self.ioctl(IOCTL_ADD, Some(&p), Some(&mut out))
            .map_err(|e| format!("adding {width}x{height}@{hz}: {e}"))?;
        Ok(out)
    }

    pub fn remove(&self, guid: GUID) {
        let _ = self.ioctl::<GUID, ()>(IOCTL_REMOVE, Some(&guid), None);
    }

    pub fn ping(&self) {
        let _ = self.ioctl::<(), ()>(IOCTL_PING, None, None);
    }

    fn ioctl<I, O>(&self, code: u32, input: Option<&I>, output: Option<&mut O>) -> windows::core::Result<()> {
        unsafe {
            DeviceIoControl(
                self.0,
                code,
                input.map(|i| i as *const I as *const _),
                if input.is_some() { size_of::<I>() as u32 } else { 0 },
                output.map(|o| o as *mut O as *mut _),
                if size_of::<O>() > 0 { size_of::<O>() as u32 } else { 0 },
                None,
                None,
            )
        }
    }
}

impl Drop for Vda {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
