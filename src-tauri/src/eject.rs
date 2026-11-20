use std::path::Path;

#[cfg(target_os = "macos")]
use std::{fs, process::Command};

#[cfg(windows)]
use std::{ffi::c_void, mem, ptr};

#[cfg(windows)]
use windows_sys::Win32::{
    Devices::DeviceAndDriverInstallation::{
        CM_Request_Device_EjectW, CR_SUCCESS, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO,
        PNP_VETO_TYPE, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
        SP_DEVINFO_DATA, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
        SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
    },
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDriveTypeW,
        OPEN_EXISTING,
    },
    System::{
        IO::DeviceIoControl,
        Ioctl::{GUID_DEVINTERFACE_DISK, IOCTL_STORAGE_GET_DEVICE_NUMBER, STORAGE_DEVICE_NUMBER},
    },
};

use crate::error::{AppError, AppResult};

#[cfg(any(target_os = "macos", test))]
fn is_macos_volume_root(path: &Path) -> bool {
    path.parent() == Some(Path::new("/Volumes"))
        && path.file_name().is_some_and(|name| !name.is_empty())
}

#[cfg(target_os = "macos")]
pub fn eject_volume(source_root: &Path) -> AppResult<()> {
    let canonical =
        fs::canonicalize(source_root).map_err(|error| AppError::io(source_root, error))?;
    if !is_macos_volume_root(&canonical) {
        return Err(AppError::UnsafeEjectPath(canonical));
    }

    let output = Command::new("/usr/sbin/diskutil")
        .arg("eject")
        .arg(&canonical)
        .output()
        .map_err(|error| AppError::io("/usr/sbin/diskutil", error))?;
    if output.status.success() {
        return Ok(());
    }

    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(AppError::EjectFailed(if detail.is_empty() {
        format!("diskutil 結束狀態：{}", output.status)
    } else {
        detail
    }))
}

#[cfg(any(windows, test))]
fn windows_drive_letter(value: &str) -> Option<char> {
    let bytes = value.as_bytes();
    if bytes.len() == 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
    {
        Some((bytes[0] as char).to_ascii_uppercase())
    } else {
        None
    }
}

#[cfg(windows)]
struct OwnedHandle(HANDLE);

#[cfg(windows)]
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: OwnedHandle is only constructed from a valid CreateFileW handle.
        unsafe { CloseHandle(self.0) };
    }
}

#[cfg(windows)]
struct OwnedDeviceInfo(HDEVINFO);

#[cfg(windows)]
impl Drop for OwnedDeviceInfo {
    fn drop(&mut self) {
        // SAFETY: OwnedDeviceInfo only wraps a valid SetupDiGetClassDevsW result.
        unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

#[cfg(windows)]
fn storage_device_number(handle: HANDLE) -> std::io::Result<STORAGE_DEVICE_NUMBER> {
    let mut returned = 0;
    // SAFETY: zero is a valid initial representation for this plain Win32 output struct.
    let mut number: STORAGE_DEVICE_NUMBER = unsafe { mem::zeroed() };
    // SAFETY: handle is valid; number points to a correctly sized output buffer;
    // the synchronous call takes no input buffer or OVERLAPPED value.
    let succeeded = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            ptr::null::<c_void>(),
            0,
            ptr::addr_of_mut!(number).cast(),
            mem::size_of::<STORAGE_DEVICE_NUMBER>() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if succeeded == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(number)
    }
}

#[cfg(windows)]
fn open_device(path: *const u16) -> std::io::Result<OwnedHandle> {
    // SAFETY: path must point to a NUL-terminated Win32 device path. Zero desired
    // access is sufficient for IOCTL_STORAGE_GET_DEVICE_NUMBER and avoids DASD
    // administrator requirements. OwnedHandle closes a successful result.
    let handle = unsafe {
        CreateFileW(
            path,
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(OwnedHandle(handle))
    }
}

#[cfg(windows)]
fn find_disk_device_instance(volume_number: STORAGE_DEVICE_NUMBER) -> std::io::Result<u32> {
    // SAFETY: GUID and flags are valid; optional enumerator and parent window are null.
    let raw_set = unsafe {
        SetupDiGetClassDevsW(
            &GUID_DEVINTERFACE_DISK,
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if raw_set == -1 {
        return Err(std::io::Error::last_os_error());
    }
    let set = OwnedDeviceInfo(raw_set);

    for index in 0.. {
        // SAFETY: zero is a valid initial representation after cbSize is populated.
        let mut interface: SP_DEVICE_INTERFACE_DATA = unsafe { mem::zeroed() };
        interface.cbSize = mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        // SAFETY: set and GUID are valid and interface is a correctly sized output.
        if unsafe {
            SetupDiEnumDeviceInterfaces(
                set.0,
                ptr::null(),
                &GUID_DEVINTERFACE_DISK,
                index,
                &mut interface,
            )
        } == 0
        {
            break;
        }

        let mut required_size = 0;
        // SAFETY: the first call intentionally provides no detail buffer so Windows
        // returns the required allocation size.
        unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                set.0,
                &interface,
                ptr::null_mut(),
                0,
                &mut required_size,
                ptr::null_mut(),
            )
        };
        if required_size < mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 {
            continue;
        }

        // A usize buffer guarantees alignment for the variable-length detail struct.
        let unit_count = (required_size as usize).div_ceil(mem::size_of::<usize>());
        let mut detail_storage = vec![0usize; unit_count];
        let detail = detail_storage
            .as_mut_ptr()
            .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        // SAFETY: detail points to aligned storage of at least required_size bytes.
        unsafe {
            (*detail).cbSize = mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        }
        // SAFETY: zero is a valid initial representation after cbSize is populated.
        let mut device_info: SP_DEVINFO_DATA = unsafe { mem::zeroed() };
        device_info.cbSize = mem::size_of::<SP_DEVINFO_DATA>() as u32;
        // SAFETY: all buffers are correctly sized and remain alive for this call.
        if unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                set.0,
                &interface,
                detail,
                required_size,
                ptr::null_mut(),
                &mut device_info,
            )
        } == 0
        {
            continue;
        }

        // SAFETY: successful SetupDiGetDeviceInterfaceDetailW wrote a NUL-terminated
        // DevicePath inside detail_storage, which remains alive while it is opened.
        let device_path = unsafe { ptr::addr_of!((*detail).DevicePath).cast::<u16>() };
        let disk = match open_device(device_path) {
            Ok(handle) => handle,
            Err(_) => continue,
        };
        let disk_number = match storage_device_number(disk.0) {
            Ok(number) => number,
            Err(_) => continue,
        };
        if disk_number.DeviceType == volume_number.DeviceType
            && disk_number.DeviceNumber == volume_number.DeviceNumber
        {
            return Ok(device_info.DevInst);
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "找不到與磁碟代號相符的即插即用裝置",
    ))
}

#[cfg(windows)]
pub fn eject_volume(source_root: &Path) -> AppResult<()> {
    const DRIVE_REMOVABLE: u32 = 2;

    let source_text = source_root.to_string_lossy();
    let letter = windows_drive_letter(&source_text)
        .ok_or_else(|| AppError::UnsafeEjectPath(source_root.to_path_buf()))?;
    let root_wide = [letter as u16, b':' as u16, b'\\' as u16, 0];
    // SAFETY: root_wide is a valid, NUL-terminated drive-root string.
    if unsafe { GetDriveTypeW(root_wide.as_ptr()) } != DRIVE_REMOVABLE {
        return Err(AppError::UnsafeEjectPath(source_root.to_path_buf()));
    }

    let device_path: Vec<u16> = format!(r"\\.\{letter}:")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let volume = open_device(device_path.as_ptr()).map_err(|error| {
        AppError::EjectFailed(format!("無法識別 {letter}: 記憶卡裝置：{error}"))
    })?;
    let volume_number = storage_device_number(volume.0).map_err(|error| {
        AppError::EjectFailed(format!("無法取得 {letter}: 記憶卡裝置編號：{error}"))
    })?;
    let device_instance = find_disk_device_instance(volume_number).map_err(|error| {
        AppError::EjectFailed(format!("無法找到 {letter}: 的安全移除裝置：{error}"))
    })?;
    drop(volume);

    let mut veto_type: PNP_VETO_TYPE = 0;
    let mut veto_name = [0u16; 260];
    // SAFETY: device_instance came from the live SetupAPI device set; veto outputs
    // point to valid writable storage and flags are required to be zero.
    let result = unsafe {
        CM_Request_Device_EjectW(
            device_instance,
            &mut veto_type,
            veto_name.as_mut_ptr(),
            veto_name.len() as u32,
            0,
        )
    };
    if result != CR_SUCCESS {
        let end = veto_name
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(veto_name.len());
        let veto_detail = String::from_utf16_lossy(&veto_name[..end]);
        let suffix = if veto_detail.is_empty() {
            String::new()
        } else {
            format!("（{veto_detail}）")
        };
        return Err(AppError::EjectFailed(format!(
            "Windows 拒絕安全退出 {letter}:，代碼 {result}、占用類型 {veto_type}{suffix}"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_accepts_direct_children_of_volumes() {
        assert!(is_macos_volume_root(Path::new("/Volumes/CAMERA")));
        assert!(is_macos_volume_root(Path::new("/Volumes/Camera Card")));
        assert!(!is_macos_volume_root(Path::new("/Volumes")));
        assert!(!is_macos_volume_root(Path::new("/Volumes/CAMERA/DCIM")));
        assert!(!is_macos_volume_root(Path::new("/")));
        assert!(!is_macos_volume_root(Path::new("/tmp/test-card")));
    }

    #[test]
    fn only_accepts_windows_drive_roots() {
        assert_eq!(windows_drive_letter(r"E:\"), Some('E'));
        assert_eq!(windows_drive_letter("f:/"), Some('F'));
        assert_eq!(windows_drive_letter(r"C:\Windows"), None);
        assert_eq!(windows_drive_letter(r"\\server\share"), None);
        assert_eq!(windows_drive_letter("/"), None);
    }
}
