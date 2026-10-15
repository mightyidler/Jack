//! Live process identity and verified Authenticode publisher; no name guessing.
use jack_core::AppIdentity;
use std::path::Path;
use windows::{
    Win32::{
        Foundation::*,
        Security::{Cryptography::*, WinTrust::*},
        Storage::Packaging::Appx::GetApplicationUserModelId,
        System::Threading::*,
    },
    core::{PCWSTR, PWSTR},
};

struct Process(HANDLE);
impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
pub fn process_identity(pid: u32) -> windows::core::Result<AppIdentity> {
    let process = Process(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)? });
    let mut path = vec![0u16; 32768];
    let mut length = path.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &mut length,
        )?;
    }
    let path = String::from_utf16_lossy(&path[..length as usize]);
    let mut app_id = vec![0u16; 1024];
    let mut app_len = app_id.len() as u32;
    let status = unsafe {
        GetApplicationUserModelId(process.0, &mut app_len, Some(PWSTR(app_id.as_mut_ptr())))
    };
    let aumid = if status == ERROR_SUCCESS && app_len > 1 {
        Some(String::from_utf16_lossy(&app_id[..(app_len as usize - 1)]))
    } else {
        None
    };
    let exe = Path::new(&path)
        .file_name()
        .map(|p| p.to_string_lossy().to_string());
    // Only known automatic classification candidates need signature work.
    let publisher = exe
        .as_deref()
        .filter(|name| {
            matches!(
                name.to_ascii_lowercase().as_str(),
                "valorant-win64-shipping.exe" | "discord.exe" | "spotify.exe"
            )
        })
        .and_then(|_| verified_publisher(&path));
    Ok(AppIdentity {
        stable_id: aumid
            .as_ref()
            .map(|id| format!("aumid:{id}"))
            .or_else(|| {
                publisher
                    .as_ref()
                    .zip(exe.as_ref())
                    .map(|(publisher, exe)| format!("publisher:{publisher}:{}", exe.to_lowercase()))
            })
            .unwrap_or_else(|| format!("path:{}", path.replace('\\', "/").to_lowercase())),
        executable_name: exe,
        verified_publisher: publisher,
        app_user_model_id: aumid,
    })
}

pub fn verified_publisher(path: &str) -> Option<String> {
    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(wide.as_ptr()),
        ..Default::default()
    };
    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let result = unsafe {
        if WinVerifyTrust(
            HWND(INVALID_HANDLE_VALUE.0),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        ) != 0
        {
            None
        } else {
            let provider = WTHelperProvDataFromStateData(data.hWVTStateData);
            let signer = if provider.is_null() {
                std::ptr::null_mut()
            } else {
                WTHelperGetProvSignerFromChain(provider, 0, false, 0)
            };
            let certificate = if signer.is_null() {
                std::ptr::null_mut()
            } else {
                WTHelperGetProvCertFromChain(signer, 0)
            };
            if certificate.is_null() {
                None
            } else {
                let mut name = vec![0u16; 1024];
                let length = CertGetNameStringW(
                    (*certificate).pCert,
                    CERT_NAME_SIMPLE_DISPLAY_TYPE,
                    0,
                    None,
                    Some(&mut name),
                );
                (length > 1 && length as usize <= name.len())
                    .then(|| String::from_utf16_lossy(&name[..length as usize - 1]))
            }
        }
    };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe {
        let _ = WinVerifyTrust(
            HWND(INVALID_HANDLE_VALUE.0),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        );
    }
    result
}
