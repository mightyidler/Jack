//! Actual SetupAPI root-device lifecycle for per-group Jack render sources.
//! One UUID-addressed PDO loads one JackAudio WaveRT endpoint. No fake endpoint
//! IDs are returned. Driver-store/kernel trust and elevation remain OS checks.
use serde::Serialize;
use std::{error::Error, path::Path};
use uuid::Uuid;
use windows::{
    Win32::{
        Devices::DeviceAndDriverInstallation::*, Foundation::ERROR_NO_MORE_ITEMS,
        UI::Shell::IsUserAnAdmin,
    },
    core::{BOOL, HSTRING, PCWSTR},
};

const HARDWARE: &str = "Root\\Jack_Audio";
const SERVICE: &str = "JackAudio";
const PREFIX: &str = "ROOT\\JACK_AUDIO\\";
type DeviceResult<T> = Result<T, Box<dyn Error>>;
struct DeviceSet(HDEVINFO);
impl Drop for DeviceSet {
    fn drop(&mut self) {
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}
fn info() -> SP_DEVINFO_DATA {
    SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    }
}
fn instance(group: Uuid) -> DeviceResult<String> {
    if group.is_nil() {
        return Err("empty group UUID".into());
    }
    Ok(format!("{PREFIX}{group}"))
}
fn property(
    set: &DeviceSet,
    device: &SP_DEVINFO_DATA,
    key: SETUP_DI_REGISTRY_PROPERTY,
) -> DeviceResult<Vec<String>> {
    let mut bytes = vec![0u8; 65536];
    let mut required = 0;
    unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            set.0,
            device,
            key,
            None,
            Some(&mut bytes),
            Some(&mut required),
        )?;
    }
    if required as usize > bytes.len() || !required.is_multiple_of(2) {
        return Err("invalid device property length".into());
    }
    let wide: Vec<_> = bytes[..required as usize]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    wide.split(|c| *c == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf16(s).map_err(Into::into))
        .collect()
}
fn set_text(
    set: &DeviceSet,
    device: &mut SP_DEVINFO_DATA,
    key: SETUP_DI_REGISTRY_PROPERTY,
    text: &str,
    multi: bool,
) -> DeviceResult<()> {
    let mut wide: Vec<_> = text.encode_utf16().chain([0]).collect();
    if multi {
        wide.push(0);
    }
    let bytes: Vec<_> = wide.iter().flat_map(|c| c.to_le_bytes()).collect();
    unsafe {
        SetupDiSetDeviceRegistryPropertyW(set.0, device, key, Some(&bytes))?;
    }
    Ok(())
}
#[derive(Debug, Serialize)]
pub struct GroupDevice {
    pub group: Uuid,
    pub instance_id: String,
    pub name: String,
    pub service: Option<String>,
    pub device_started: bool,
    pub problem_code: u32,
}
fn describe(set: &DeviceSet, device: &SP_DEVINFO_DATA) -> DeviceResult<GroupDevice> {
    let mut id = [0u16; 256];
    unsafe {
        SetupDiGetDeviceInstanceIdW(set.0, device, Some(&mut id), None)?;
    }
    let end = id.iter().position(|c| *c == 0).unwrap_or(id.len());
    let id = String::from_utf16(&id[..end])?;
    let upper = id.to_ascii_uppercase();
    let suffix = upper
        .strip_prefix(PREFIX)
        .ok_or("device is outside Jack root namespace")?;
    let group = Uuid::parse_str(suffix)?;
    if group.is_nil() || device.ClassGuid != GUID_DEVCLASS_MEDIA {
        return Err("invalid Jack device identity/class".into());
    }
    if !property(set, device, SPDRP_HARDWAREID)?
        .iter()
        .any(|s| s.eq_ignore_ascii_case(HARDWARE))
    {
        return Err("device hardware ID is not Jack".into());
    }
    let service = property(set, device, SPDRP_SERVICE)
        .ok()
        .and_then(|v| v.into_iter().next());
    if service
        .as_deref()
        .is_some_and(|s| !s.eq_ignore_ascii_case(SERVICE))
    {
        return Err("Jack namespace is bound to another driver; refusing mutation".into());
    }
    let (mut status, mut problem) = (CM_DEVNODE_STATUS_FLAGS(0), CM_PROB(0));
    let result = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, device.DevInst, 0) };
    if result != CR_SUCCESS {
        return Err(format!("PnP status unavailable: {}", result.0).into());
    }
    Ok(GroupDevice {
        group,
        instance_id: id,
        name: property(set, device, SPDRP_FRIENDLYNAME)
            .ok()
            .and_then(|v| v.into_iter().next())
            .unwrap_or_default(),
        device_started: status.0 & DN_STARTED.0 != 0
            && problem.0 == 0
            && service
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(SERVICE)),
        service,
        problem_code: problem.0,
    })
}
fn admin() -> DeviceResult<()> {
    if !unsafe { IsUserAnAdmin() }.as_bool() {
        return Err("Jack source device creation requires administrator elevation".into());
    }
    Ok(())
}
fn remove_device(set: &DeviceSet, device: &SP_DEVINFO_DATA) -> DeviceResult<()> {
    let params = SP_REMOVEDEVICE_PARAMS {
        ClassInstallHeader: SP_CLASSINSTALL_HEADER {
            cbSize: size_of::<SP_CLASSINSTALL_HEADER>() as u32,
            InstallFunction: DIF_REMOVE,
        },
        Scope: DI_REMOVEDEVICE_GLOBAL,
        HwProfile: 0,
    };
    unsafe {
        SetupDiSetClassInstallParamsW(
            set.0,
            Some(device),
            Some(&params.ClassInstallHeader),
            size_of::<SP_REMOVEDEVICE_PARAMS>() as u32,
        )?;
        SetupDiCallClassInstaller(DIF_REMOVE, set.0, Some(device))?;
    }
    Ok(())
}

/// Read actual PnP devices, never synthesize default groups as installed sources.
pub fn list() -> DeviceResult<Vec<GroupDevice>> {
    let set = DeviceSet(unsafe {
        SetupDiGetClassDevsW(
            Some(&GUID_DEVCLASS_MEDIA),
            PCWSTR::null(),
            None,
            DIGCF_PRESENT,
        )?
    });
    let mut result = vec![];
    for index in 0..u32::MAX {
        let mut device = info();
        match unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut device) } {
            Ok(()) => {}
            Err(e) if e.code() == windows::core::HRESULT::from_win32(ERROR_NO_MORE_ITEMS.0) => {
                break;
            }
            Err(e) => return Err(e.into()),
        }
        let mut id = [0u16; 256];
        unsafe {
            SetupDiGetDeviceInstanceIdW(set.0, &device, Some(&mut id), None)?;
        }
        let end = id.iter().position(|c| *c == 0).unwrap_or(id.len());
        if String::from_utf16(&id[..end])?
            .to_ascii_uppercase()
            .starts_with(PREFIX)
        {
            result.push(describe(&set, &device)?);
        }
    }
    Ok(result)
}
#[derive(Debug, Serialize)]
pub struct EnsureResult {
    pub device: GroupDevice,
    pub created: bool,
    pub reboot_required: bool,
}
/// Requires an already staged, OS-trusted JackAudio package. UUID is stable across
/// restarts. Existing non-Jack drivers/devices are never rebound or overwritten.
pub fn ensure(group: Uuid, name: &str) -> DeviceResult<EnsureResult> {
    let id = instance(group)?;
    if name.trim().is_empty() || name.contains('\0') || name.encode_utf16().count() > 128 {
        return Err("invalid group name".into());
    }
    admin()?;
    let set = DeviceSet(unsafe { SetupDiCreateDeviceInfoList(Some(&GUID_DEVCLASS_MEDIA), None)? });
    let mut device = info();
    let id_wide = HSTRING::from(&id);
    let friendly = format!("Jack {name}");
    let exists =
        unsafe { SetupDiOpenDeviceInfoW(set.0, &id_wide, None, 0, Some(&mut device)) }.is_ok();
    if exists {
        let existing = describe(&set, &device)?;
        if existing.name != friendly {
            set_text(&set, &mut device, SPDRP_FRIENDLYNAME, &friendly, false)?;
        }
    } else {
        unsafe {
            SetupDiCreateDeviceInfoW(
                set.0,
                &id_wide,
                &GUID_DEVCLASS_MEDIA,
                &HSTRING::from(&friendly),
                None,
                SETUP_DI_DEVICE_CREATION_FLAGS(0),
                Some(&mut device),
            )?;
        }
        set_text(&set, &mut device, SPDRP_HARDWAREID, HARDWARE, true)?;
        set_text(&set, &mut device, SPDRP_FRIENDLYNAME, &friendly, false)?;
        unsafe {
            SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set.0, Some(&device))?;
        }
    }
    let mut reboot = BOOL::default();
    if let Err(error) = unsafe {
        DiInstallDevice(
            None,
            set.0,
            &device,
            None,
            DIIDFLAG_NOFINISHINSTALLUI,
            Some(&mut reboot),
        )
    } {
        if !exists {
            let _ = remove_device(&set, &device);
        }
        return Err(error.into());
    }
    Ok(EnsureResult {
        device: describe(&set, &device)?,
        created: !exists,
        reboot_required: reboot.as_bool(),
    })
}
/// Caller must restore owned application routes and stop capture before removal.
pub fn remove(group: Uuid) -> DeviceResult<()> {
    let id = instance(group)?;
    admin()?;
    let set = DeviceSet(unsafe { SetupDiCreateDeviceInfoList(Some(&GUID_DEVCLASS_MEDIA), None)? });
    let mut device = info();
    unsafe {
        SetupDiOpenDeviceInfoW(set.0, &HSTRING::from(id), None, 0, Some(&mut device))?;
    }
    let _owned = describe(&set, &device)?;
    remove_device(&set, &device)
}
/// Stage the actual package using the Windows installer. No boot policy/certificate
/// trust changes or automatic reboot. OS rejects an untrusted kernel package.
pub fn install_package(inf: &Path) -> DeviceResult<bool> {
    admin()?;
    let inf = inf.canonicalize()?;
    if !inf
        .file_name()
        .is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case("JackAudio.inf"))
    {
        return Err("expected JackAudio.inf".into());
    }
    let bytes = std::fs::read(&inf)?;
    let text = if bytes.starts_with(&[0xff, 0xfe]) {
        String::from_utf16(
            &bytes[2..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect::<Vec<_>>(),
        )?
    } else {
        String::from_utf8(bytes)?
    };
    if !text.to_ascii_lowercase().contains("root\\jack_audio")
        || !text.contains("AddService=JackAudio,")
    {
        return Err("package does not declare the Jack source driver".into());
    }
    let mut reboot = BOOL::default();
    unsafe {
        DiInstallDriverW(
            None,
            &HSTRING::from(inf.as_os_str()),
            DIINSTALLDRIVER_FLAGS(0),
            Some(&mut reboot),
        )?;
    }
    Ok(reboot.as_bool())
}
#[derive(Debug, Serialize)]
pub struct DefaultSources {
    pub devices: Vec<EnsureResult>,
    pub reboot_required: bool,
    pub all_devices_started: bool,
}
pub fn install_defaults(inf: &Path) -> DeviceResult<DefaultSources> {
    let mut reboot_required = install_package(inf)?;
    let mut devices = vec![];
    for group in jack_core::Settings::default().groups {
        let result = ensure(group.id, &group.display_name)?;
        reboot_required |= result.reboot_required;
        devices.push(result);
    }
    let all_devices_started = devices.iter().all(|d| d.device.device_started);
    Ok(DefaultSources {
        devices,
        reboot_required,
        all_devices_started,
    })
}
