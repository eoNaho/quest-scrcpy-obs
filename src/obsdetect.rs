//! Detects whether OBS Studio is currently running, so the UI can auto-hide
//! sensitive info (device serials, saved Wi-Fi addresses) while it's open —
//! in case OBS captures this window (Window/Display Capture).

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    /// True if an `obsXX.exe` process is running (OBS Studio's process name
    /// on both 64- and 32-bit builds).
    pub fn is_running() -> bool {
        unsafe {
            let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return false;
            };
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            let mut found = false;
            if Process32FirstW(snapshot, &mut entry).is_ok() {
                loop {
                    let len = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len());
                    let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                    if name.eq_ignore_ascii_case("obs64.exe") || name.eq_ignore_ascii_case("obs32.exe") {
                        found = true;
                        break;
                    }
                    if Process32NextW(snapshot, &mut entry).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snapshot);
            found
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn is_running() -> bool {
        false
    }
}

pub use imp::is_running;
