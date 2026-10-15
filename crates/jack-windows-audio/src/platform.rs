use serde::Serialize;
use std::{marker::PhantomData, rc::Rc};
use windows::{
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Foundation::CloseHandle,
        Media::Audio::{Endpoints::IAudioEndpointVolume, *},
        System::{
            Com::{
                CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
                CoUninitialize, STGM_READ,
                StructuredStorage::{
                    PROPVARIANT, PropVariantClear, PropVariantToStringAlloc, PropVariantToUInt32,
                },
            },
            Threading::{
                OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                QueryFullProcessImageNameW,
            },
        },
    },
    core::{Interface, PWSTR, Result},
};

/// COM is initialized and torn down on the same thread; this guard is !Send/!Sync.
pub(super) struct Apartment(PhantomData<Rc<()>>);
impl Apartment {
    pub(super) fn new() -> Result<Self> {
        // SAFETY: initialization is balanced by this non-transferable guard.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        Ok(Self(PhantomData))
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

pub(super) fn owned_string(value: PWSTR) -> Result<String> {
    if value.is_null() {
        return Err(windows::core::Error::from_hresult(
            windows::Win32::Foundation::E_POINTER,
        ));
    }
    // SAFETY: callers pass CoTaskMem-allocated strings returned by Core Audio.
    let result = unsafe { value.to_string() };
    unsafe {
        CoTaskMemFree(Some(value.0.cast()));
    }
    Ok(result?)
}
pub(super) struct Property(pub(super) PROPVARIANT);
impl Drop for Property {
    fn drop(&mut self) {
        unsafe {
            let _ = PropVariantClear(&mut self.0);
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Endpoint {
    pub id: String,
    pub name: String,
    pub flow: &'static str,
    pub form_factor: Option<u32>,
    pub defaults: Vec<&'static str>,
    pub mix_format: Option<MixFormat>,
    pub volume: Option<EndpointVolume>,
}
#[derive(Debug, Serialize)]
pub struct EndpointVolume {
    pub level: f32,
    pub muted: bool,
}
#[derive(Debug, Serialize)]
pub struct MixFormat {
    pub format_tag: u16,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
}
#[derive(Debug, Serialize)]
pub struct Session {
    pub endpoint_id: String,
    pub instance_id: String,
    pub display_name: Option<String>,
    pub pid: u32,
    pub executable_path: Option<String>,
    /// No durable identity for system/unknown processes. Never fall back to PID.
    pub stable_app_id: Option<String>,
    pub state: i32,
}
#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub endpoints: Vec<Endpoint>,
    pub sessions: Vec<Session>,
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor: Option<crate::MonitorDiagnostics>,
}

pub fn snapshot() -> Result<Snapshot> {
    let _apartment = Apartment::new()?;
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    read_snapshot(&enumerator)
}

pub(super) fn read_snapshot(enumerator: &IMMDeviceEnumerator) -> Result<Snapshot> {
    let mut result = Snapshot {
        endpoints: vec![],
        sessions: vec![],
        warnings: vec![],
        monitor: None,
    };
    // SAFETY: COM is initialized on this thread; all interfaces stay in this call.
    unsafe {
        for (flow, flow_name) in [(eRender, "render"), (eCapture, "capture")] {
            let mut defaults = vec![];
            for (role, name) in [
                (eConsole, "console"),
                (eMultimedia, "multimedia"),
                (eCommunications, "communications"),
            ] {
                match enumerator
                    .GetDefaultAudioEndpoint(flow, role)
                    .and_then(|d| owned_string(d.GetId()?))
                {
                    Ok(id) => defaults.push((name, id)),
                    Err(e) => result
                        .warnings
                        .push(format!("default {flow_name}/{name}: {e}")),
                }
            }
            let devices = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
            for index in 0..devices.GetCount()? {
                let device = devices.Item(index)?;
                let id = owned_string(device.GetId()?)?;
                let properties = device.OpenPropertyStore(STGM_READ)?;
                let friendly = Property(properties.GetValue(&PKEY_Device_FriendlyName)?);
                let name = owned_string(PropVariantToStringAlloc(&friendly.0)?)?;
                let form_factor = properties
                    .GetValue(&PKEY_AudioEndpoint_FormFactor)
                    .ok()
                    .and_then(|p| {
                        let property = Property(p);
                        PropVariantToUInt32(&property.0).ok()
                    });
                let mix_format = match device
                    .Activate::<IAudioClient>(CLSCTX_ALL, None)
                    .and_then(|c| c.GetMixFormat())
                {
                    Ok(ptr) => {
                        // GetMixFormat returns a non-null CoTaskMem allocation, including any extension.
                        let f = ptr.read_unaligned();
                        CoTaskMemFree(Some(ptr.cast()));
                        Some(MixFormat {
                            format_tag: f.wFormatTag,
                            sample_rate: f.nSamplesPerSec,
                            channels: f.nChannels,
                            bits_per_sample: f.wBitsPerSample,
                        })
                    }
                    Err(e) => {
                        result.warnings.push(format!("mix format {id}: {e}"));
                        None
                    }
                };
                if flow == eRender {
                    match read_sessions(&device, &id) {
                        Ok(sessions) => result.sessions.extend(sessions),
                        Err(e) => result.warnings.push(format!("sessions {id}: {e}")),
                    }
                }
                let volume = match device
                    .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
                    .and_then(|v| {
                        Ok(EndpointVolume {
                            level: v.GetMasterVolumeLevelScalar()?,
                            muted: v.GetMute()?.as_bool(),
                        })
                    }) {
                    Ok(volume) => Some(volume),
                    Err(e) => {
                        result.warnings.push(format!("endpoint volume {id}: {e}"));
                        None
                    }
                };
                result.endpoints.push(Endpoint {
                    defaults: defaults
                        .iter()
                        .filter(|(_, default)| default == &id)
                        .map(|(name, _)| *name)
                        .collect(),
                    id,
                    name,
                    flow: flow_name,
                    form_factor,
                    mix_format,
                    volume,
                });
            }
        }
    }
    Ok(result)
}

fn read_sessions(device: &IMMDevice, endpoint_id: &str) -> Result<Vec<Session>> {
    unsafe {
        let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
        let enumerator = manager.GetSessionEnumerator()?;
        let count = enumerator.GetCount()?;
        let mut sessions = vec![];
        for index in 0..count {
            let control = enumerator.GetSession(index)?;
            sessions.push(read_session(&control, endpoint_id)?);
        }
        Ok(sessions)
    }
}
pub(super) fn read_session(control: &IAudioSessionControl, endpoint_id: &str) -> Result<Session> {
    unsafe {
        let extended: IAudioSessionControl2 = control.cast()?;
        let pid = extended.GetProcessId()?;
        let executable_path = process_path(pid);
        let stable_app_id = executable_path
            .as_ref()
            .map(|path| format!("path:{}", path.replace('\\', "/").to_lowercase()));
        Ok(Session {
            endpoint_id: endpoint_id.into(),
            instance_id: owned_string(extended.GetSessionInstanceIdentifier()?)?,
            display_name: control
                .GetDisplayName()
                .ok()
                .and_then(|s| owned_string(s).ok())
                .filter(|s| !s.is_empty()),
            pid,
            executable_path,
            stable_app_id,
            state: control.GetState()?.0,
        })
    }
}
fn process_path(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut path = vec![0u16; 32768];
        let mut length = path.len() as u32;
        let queried = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &mut length,
        );
        let _ = CloseHandle(process);
        queried.ok()?;
        Some(String::from_utf16_lossy(&path[..length as usize]))
    }
}
