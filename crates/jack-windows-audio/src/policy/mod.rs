//! Public reads + isolated undocumented Windows default/app policy writes.
//! This adapter is MTA thread-confined. PID bindings are live only, never journal keys.
mod bindings;
mod verification;
use crate::platform::{Apartment, owned_string};
use bindings::*;
use jack_core::routing::{AudioPolicy, Flow, Role, RouteKey, RoutingError, WriteOutcome};
use serde::Serialize;
use std::{collections::BTreeMap, marker::PhantomData, rc::Rc};
pub use verification::{
    PolicyVerification, recover_policy_journal, verify_default_roundtrip, verify_self_app_roundtrip,
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT},
        Media::Audio::*,
        Storage::Packaging::Appx::GetApplicationUserModelId,
        System::{
            Com::{CLSCTX_ALL, CoCreateInstance},
            Threading::{
                OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, WaitForSingleObject,
            },
            WinRT::{RO_INIT_MULTITHREADED, RoGetActivationFactory, RoInitialize, RoUninitialize},
        },
    },
    core::{HSTRING, PWSTR},
};

const PREFIX: &str = r"\\?\SWD#MMDEVAPI#";
const RENDER_SUFFIX: &str = "#{e6327cad-dcec-4949-ae8a-991e976a79d2}";
const CAPTURE_SUFFIX: &str = "#{2eef81be-33fa-4800-9670-1cd474972c3f}";
fn flow(flow: Flow) -> EDataFlow {
    match flow {
        Flow::Render => eRender,
        Flow::Capture => eCapture,
    }
}
fn role(role: Role) -> ERole {
    match role {
        Role::Console => eConsole,
        Role::Multimedia => eMultimedia,
        Role::Communications => eCommunications,
    }
}
fn error(error: impl std::fmt::Display) -> RoutingError {
    RoutingError::Policy(error.to_string())
}
fn packed_endpoint(endpoint: &str, value: Flow) -> String {
    format!(
        "{PREFIX}{endpoint}{}",
        match value {
            Flow::Render => RENDER_SUFFIX,
            Flow::Capture => CAPTURE_SUFFIX,
        }
    )
}
fn unpack_endpoint(value: &str, flow: Flow) -> Result<Option<String>, RoutingError> {
    if value.is_empty() {
        return Ok(None);
    }
    let suffix = match flow {
        Flow::Render => RENDER_SUFFIX,
        Flow::Capture => CAPTURE_SUFFIX,
    };
    let id = value
        .strip_prefix(PREFIX)
        .and_then(|s| s.strip_suffix(suffix))
        .filter(|s| !s.is_empty());
    id.map(|s| Some(s.into()))
        .ok_or_else(|| error("unrecognized app endpoint encoding; no route was written"))
}
struct WinRtApartment(PhantomData<Rc<()>>);
impl WinRtApartment {
    fn new() -> windows::core::Result<Self> {
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)?;
        }
        Ok(Self(PhantomData))
    }
}
impl Drop for WinRtApartment {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}
struct ProcessBinding {
    pid: u32,
    handle: HANDLE,
    app_id: Option<String>,
}
impl Drop for ProcessBinding {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PolicyCapabilities {
    pub default_interface_available: bool,
    pub app_interface_variant: Option<&'static str>,
    pub app_interface_error: Option<String>,
    pub atomic_compare_and_set: bool,
    pub interface_is_undocumented: bool,
}
pub struct WindowsAudioPolicy {
    enumerator: IMMDeviceEnumerator,
    defaults: PolicyConfig,
    apps: Option<AppPolicy>,
    app_error: Option<String>,
    bindings: BTreeMap<String, Vec<ProcessBinding>>,
    _winrt: WinRtApartment,
    _apartment: Apartment,
}
impl WindowsAudioPolicy {
    pub fn new() -> Result<Self, RoutingError> {
        let apartment = Apartment::new().map_err(error)?;
        let winrt = WinRtApartment::new().map_err(error)?;
        let enumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(error)?;
        let defaults =
            unsafe { CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL) }.map_err(error)?;
        let class = HSTRING::from("Windows.Media.Internal.AudioPolicyConfig");
        let (apps, app_error) = match unsafe { RoGetActivationFactory::<AppPolicy21H2>(&class) } {
            Ok(factory) => (Some(AppPolicy::Modern(factory)), None),
            Err(modern_error) => {
                match unsafe { RoGetActivationFactory::<AppPolicyDownlevel>(&class) } {
                    Ok(factory) => (Some(AppPolicy::Downlevel(factory)), None),
                    Err(legacy_error) => (
                        None,
                        Some(format!("21h2: {modern_error}; downlevel: {legacy_error}")),
                    ),
                }
            }
        };
        Ok(Self {
            enumerator,
            defaults,
            apps,
            app_error,
            bindings: BTreeMap::new(),
            _winrt: winrt,
            _apartment: apartment,
        })
    }
    pub fn capabilities(&self) -> PolicyCapabilities {
        PolicyCapabilities {
            default_interface_available: true,
            app_interface_variant: self.apps.as_ref().map(AppPolicy::variant),
            app_interface_error: self.app_error.clone(),
            atomic_compare_and_set: false,
            interface_is_undocumented: true,
        }
    }
    /// Returned durable identity is based on verified live executable path, not PID.
    pub fn bind_process(&mut self, pid: u32) -> Result<String, RoutingError> {
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                pid,
            )
        }
        .map_err(error)?;
        let mut app_id = vec![0u16; 1024];
        let mut app_length = app_id.len() as u32;
        let status = unsafe {
            GetApplicationUserModelId(handle, &mut app_length, Some(PWSTR(app_id.as_mut_ptr())))
        };
        let app_id = match status {
            windows::Win32::Foundation::ERROR_SUCCESS if app_length > 1 => {
                Some(String::from_utf16_lossy(&app_id[..app_length as usize - 1]))
            }
            windows::Win32::Foundation::APPMODEL_ERROR_NO_APPLICATION => None,
            _ => {
                unsafe {
                    let _ = CloseHandle(handle);
                }
                return Err(error("cannot verify process application identity"));
            }
        };
        let binding = ProcessBinding {
            pid,
            handle,
            app_id,
        };
        if unsafe { WaitForSingleObject(handle, 0) } != WAIT_TIMEOUT {
            return Err(error("process is not running"));
        }
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        }
        .map_err(error)?;
        let identity = format!(
            "path:{}",
            String::from_utf16_lossy(&buffer[..length as usize])
                .replace('\\', "/")
                .to_lowercase()
        );
        let group = self.bindings.entry(identity.clone()).or_default();
        group.retain(|process| unsafe { WaitForSingleObject(process.handle, 0) } == WAIT_TIMEOUT);
        if group.iter().any(|process| process.pid == pid) {
            return Ok(identity);
        }
        // Exact live executable path and package identity, plus policy consensus,
        // are required before treating several PIDs as one persistent app rule.
        for existing in group.iter() {
            if existing.app_id != binding.app_id {
                return Err(error(
                    "different application IDs share this executable; app aggregation refused",
                ));
            }
            let factory = self
                .apps
                .as_ref()
                .ok_or_else(|| error("app routing interface unavailable"))?;
            for route_flow in [Flow::Render, Flow::Capture] {
                for route_role in [Role::Console, Role::Multimedia, Role::Communications] {
                    let previous = factory
                        .get(existing.pid, flow(route_flow), role(route_role))
                        .map_err(error)?;
                    let current = factory
                        .get(pid, flow(route_flow), role(route_role))
                        .map_err(error)?;
                    if previous != current {
                        return Err(error(
                            "app processes have different persisted audio preferences; aggregation refused",
                        ));
                    }
                }
            }
        }
        group.push(binding);
        Ok(identity)
    }
    fn app(&self, identity: &str) -> Result<(&AppPolicy, Vec<u32>), RoutingError> {
        let group = self
            .bindings
            .get(identity)
            .ok_or_else(|| error("app has no verified live process binding"))?;
        let live: Vec<_> = group
            .iter()
            .filter(|process| unsafe { WaitForSingleObject(process.handle, 0) } == WAIT_TIMEOUT)
            .map(|process| process.pid)
            .collect();
        if live.is_empty() {
            return Err(error(
                "bound app exited; rebind by durable identity before restoring",
            ));
        }
        let factory = self
            .apps
            .as_ref()
            .ok_or_else(|| error("app routing interface unavailable"))?;
        Ok((factory, live))
    }
    fn validate_endpoint(&self, endpoint: &str, route_flow: Flow) -> Result<(), RoutingError> {
        if endpoint.is_empty() || endpoint.contains('\0') {
            return Err(RoutingError::InvalidJournal);
        }
        use windows::core::Interface;
        let device = unsafe { self.enumerator.GetDevice(&HSTRING::from(endpoint)) }
            .map_err(|e| error(format!("endpoint lookup {endpoint}: {e}")))?;
        if unsafe { device.GetState() }.map_err(error)? != DEVICE_STATE_ACTIVE
            || unsafe { device.cast::<IMMEndpoint>().map_err(error)?.GetDataFlow() }
                .map_err(error)?
                != flow(route_flow)
        {
            return Err(error("endpoint is inactive or has the wrong data flow"));
        }
        Ok(())
    }
}
impl AudioPolicy for WindowsAudioPolicy {
    fn read(&mut self, key: &RouteKey) -> Result<Option<String>, RoutingError> {
        match key {
            RouteKey::Default {
                flow: route_flow,
                role: route_role,
            } => {
                let device = unsafe {
                    self.enumerator
                        .GetDefaultAudioEndpoint(flow(*route_flow), role(*route_role))
                }
                .map_err(error)?;
                Ok(Some(
                    owned_string(unsafe { device.GetId() }.map_err(error)?).map_err(error)?,
                ))
            }
            RouteKey::App {
                stable_app_id,
                flow: route_flow,
                role: route_role,
            } => {
                let (factory, processes) = self.app(stable_app_id)?;
                let mut values = processes.into_iter().map(|pid| {
                    let value = factory
                        .get(pid, flow(*route_flow), role(*route_role))
                        .map_err(error)?;
                    unpack_endpoint(&value.to_string_lossy(), *route_flow)
                });
                let first = values
                    .next()
                    .ok_or_else(|| error("app group has no live processes"))??;
                for value in values {
                    if value? != first {
                        return Err(error(
                            "app processes disagree on persisted audio policy; no group write permitted",
                        ));
                    }
                }
                Ok(first)
            }
        }
    }
    fn compare_and_set(
        &mut self,
        key: &RouteKey,
        expected: Option<&str>,
        desired: Option<&str>,
    ) -> Result<WriteOutcome, RoutingError> {
        let route_flow = match key {
            RouteKey::Default { flow, .. } | RouteKey::App { flow, .. } => *flow,
        };
        if let Some(endpoint) = desired {
            self.validate_endpoint(endpoint, route_flow)?;
        }
        if self.read(key)?.as_deref() != expected {
            return Ok(WriteOutcome::Conflict);
        }
        // Windows offers no atomic CAS. This final read narrows but cannot eliminate the race.
        match key {
            RouteKey::Default {
                role: route_role, ..
            } => {
                let endpoint = desired
                    .ok_or_else(|| error("Windows cannot clear a system default endpoint"))?;
                self.defaults
                    .set_default(&HSTRING::from(endpoint), role(*route_role))
                    .map_err(error)?;
            }
            RouteKey::App {
                stable_app_id,
                role: route_role,
                ..
            } => {
                let (factory, processes) = self.app(stable_app_id)?;
                let value = desired
                    .map(|id| HSTRING::from(packed_endpoint(id, route_flow)))
                    .unwrap_or_default();
                let previous = expected
                    .map(|id| HSTRING::from(packed_endpoint(id, route_flow)))
                    .unwrap_or_default();
                let mut written = vec![];
                let operation = (|| {
                    for pid in processes {
                        let current = factory
                            .get(pid, flow(route_flow), role(*route_role))
                            .map_err(error)?;
                        // Windows can couple a persisted per-app write across PIDs.
                        if current == value {
                            continue;
                        }
                        if current != previous {
                            return Err(error("app group changed during policy write"));
                        }
                        // A failed native call may have mutated its persistent preference.
                        written.push(pid);
                        factory
                            .set(pid, flow(route_flow), role(*route_role), &value)
                            .map_err(error)?;
                    }
                    Ok::<_, RoutingError>(())
                })();
                if let Err(failure) = operation {
                    let mut pending = false;
                    for pid in written.into_iter().rev() {
                        match factory.get(pid, flow(route_flow), role(*route_role)) {
                            Ok(current) if current == value => {
                                if factory
                                    .set(pid, flow(route_flow), role(*route_role), &previous)
                                    .is_err()
                                {
                                    pending = true;
                                }
                            }
                            Ok(_) => {} // preserve an external change
                            Err(_) => pending = true,
                        }
                    }
                    return Err(error(format!(
                        "{failure}; group rollback pending={pending}"
                    )));
                }
            }
        }
        Ok(WriteOutcome::Applied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn app_endpoint_codec_is_flow_specific_and_clear_is_explicit() {
        for flow in [Flow::Render, Flow::Capture] {
            let encoded = packed_endpoint("{endpoint}", flow);
            assert_eq!(
                unpack_endpoint(&encoded, flow).unwrap(),
                Some("{endpoint}".into())
            );
        }
        assert_eq!(unpack_endpoint("", Flow::Render).unwrap(), None);
        assert!(unpack_endpoint(&packed_endpoint("id", Flow::Capture), Flow::Render).is_err());
        assert!(unpack_endpoint("unexpected", Flow::Render).is_err());
    }
    #[test]
    fn abi_offsets_preserve_native_slot_order() {
        use std::mem::{offset_of, size_of};
        assert_eq!(
            offset_of!(PolicyConfigVtbl, set_default_endpoint),
            size_of::<windows::core::IUnknown_Vtbl>() + 10 * size_of::<usize>()
        );
        assert_eq!(
            offset_of!(AppPolicyVtbl, set_endpoint),
            size_of::<windows::core::IInspectable_Vtbl>() + 19 * size_of::<usize>()
        );
    }
}
