//! Registrations and all COM reads remain on the owning MTA thread.
//! Callbacks signal changes; a bounded queue retains newly reported sessions.
use crate::Snapshot;
use crate::platform::{Apartment, owned_string, read_session, read_snapshot};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{E_POINTER, PROPERTYKEY},
        Media::Audio::{Endpoints::*, *},
        System::Com::{CLSCTX_ALL, CoCreateInstance},
    },
    core::{AgileReference, BOOL, GUID, Interface, PCWSTR, Ref, Result, implement},
};

#[derive(Default)]
struct Signals {
    dirty: AtomicBool,
    devices: AtomicU64,
    created: AtomicU64,
    sessions: AtomicU64,
    volumes: AtomicU64,
    missed: AtomicU64,
}
impl Signals {
    fn mark(&self, counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
        self.dirty.store(true, Ordering::Release);
    }
}
#[derive(Debug, Serialize)]
pub struct MonitorDiagnostics {
    pub endpoint_subscriptions: usize,
    pub session_subscriptions: usize,
    pub device_events: u64,
    pub session_created_events: u64,
    pub session_change_events: u64,
    pub endpoint_volume_events: u64,
    pub missed_session_events: u64,
}

#[implement(IMMNotificationClient)]
struct DeviceNotifications(Arc<Signals>);
impl IMMNotificationClient_Impl for DeviceNotifications_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> Result<()> {
        self.0.mark(&self.0.devices);
        Ok(())
    }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> Result<()> {
        self.0.mark(&self.0.devices);
        Ok(())
    }
    fn OnDeviceRemoved(&self, _: &PCWSTR) -> Result<()> {
        self.0.mark(&self.0.devices);
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, _: EDataFlow, _: ERole, _: &PCWSTR) -> Result<()> {
        self.0.mark(&self.0.devices);
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> Result<()> {
        self.0.mark(&self.0.devices);
        Ok(())
    }
}
#[implement(IAudioEndpointVolumeCallback)]
struct VolumeNotifications(Arc<Signals>);
impl IAudioEndpointVolumeCallback_Impl for VolumeNotifications_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        if data.is_null() {
            return Err(windows::core::Error::from_hresult(E_POINTER));
        }
        // Do not retain or dereference the borrowed variable-length notification.
        self.0.mark(&self.0.volumes);
        Ok(())
    }
}
#[implement(IAudioSessionNotification)]
struct SessionCreated {
    signals: Arc<Signals>,
    sender: SyncSender<AgileReference<IAudioSessionControl>>,
}
impl IAudioSessionNotification_Impl for SessionCreated_Impl {
    fn OnSessionCreated(&self, session: Ref<IAudioSessionControl>) -> Result<()> {
        // Retain a marshalled reference. Raw COM pointers are never sent across threads.
        let sent = session
            .as_ref()
            .and_then(|s| AgileReference::new(s).ok())
            .is_some_and(|s| self.sender.try_send(s).is_ok());
        if !sent {
            self.signals.missed.fetch_add(1, Ordering::Relaxed);
        }
        self.signals.mark(&self.signals.created);
        Ok(())
    }
}
#[implement(IAudioSessionEvents)]
struct SessionEvents {
    signals: Arc<Signals>,
    expired: Arc<AtomicBool>,
}
impl SessionEvents {
    fn changed(&self) -> Result<()> {
        self.signals.mark(&self.signals.sessions);
        Ok(())
    }
}
impl IAudioSessionEvents_Impl for SessionEvents_Impl {
    fn OnDisplayNameChanged(&self, _: &PCWSTR, _: *const GUID) -> Result<()> {
        self.changed()
    }
    fn OnIconPathChanged(&self, _: &PCWSTR, _: *const GUID) -> Result<()> {
        self.changed()
    }
    fn OnSimpleVolumeChanged(&self, _: f32, _: BOOL, _: *const GUID) -> Result<()> {
        self.changed()
    }
    fn OnChannelVolumeChanged(&self, _: u32, _: *const f32, _: u32, _: *const GUID) -> Result<()> {
        self.changed()
    }
    fn OnGroupingParamChanged(&self, _: *const GUID, _: *const GUID) -> Result<()> {
        self.changed()
    }
    fn OnStateChanged(&self, state: AudioSessionState) -> Result<()> {
        if state == AudioSessionStateExpired {
            self.expired.store(true, Ordering::Release);
        }
        self.changed()
    }
    fn OnSessionDisconnected(&self, _: AudioSessionDisconnectReason) -> Result<()> {
        self.expired.store(true, Ordering::Release);
        self.changed()
    }
}

struct VolumeRegistration {
    volume: IAudioEndpointVolume,
    callback: IAudioEndpointVolumeCallback,
}
impl VolumeRegistration {
    fn new(device: &IMMDevice, signals: Arc<Signals>) -> Result<Self> {
        unsafe {
            let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
            let callback: IAudioEndpointVolumeCallback = VolumeNotifications(signals).into();
            volume.RegisterControlChangeNotify(&callback)?;
            Ok(Self { volume, callback })
        }
    }
}
impl Drop for VolumeRegistration {
    fn drop(&mut self) {
        unsafe {
            let _ = self.volume.UnregisterControlChangeNotify(&self.callback);
        }
    }
}
struct SessionRegistration {
    control: IAudioSessionControl,
    callback: IAudioSessionEvents,
    expired: Arc<AtomicBool>,
}
impl SessionRegistration {
    fn new(control: IAudioSessionControl, signals: Arc<Signals>) -> Result<Self> {
        let expired = Arc::new(AtomicBool::new(false));
        let callback: IAudioSessionEvents = SessionEvents {
            signals,
            expired: expired.clone(),
        }
        .into();
        unsafe {
            control.RegisterAudioSessionNotification(&callback)?;
        }
        Ok(Self {
            control,
            callback,
            expired,
        })
    }
}
impl Drop for SessionRegistration {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .control
                .UnregisterAudioSessionNotification(&self.callback);
        }
    }
}
struct CreationRegistration {
    manager: IAudioSessionManager2,
    callback: IAudioSessionNotification,
}
impl Drop for CreationRegistration {
    fn drop(&mut self) {
        unsafe {
            let _ = self.manager.UnregisterSessionNotification(&self.callback);
        }
    }
}

struct EndpointWatch {
    _volume: VolumeRegistration,
    creation: Option<CreationRegistration>,
    incoming: Receiver<AgileReference<IAudioSessionControl>>,
    sessions: BTreeMap<String, SessionRegistration>,
}
impl EndpointWatch {
    fn new(device: &IMMDevice, render: bool, signals: Arc<Signals>) -> Result<Self> {
        let volume = VolumeRegistration::new(device, signals.clone())?;
        let (sender, incoming) = mpsc::sync_channel(256);
        let creation = if render {
            unsafe {
                let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
                let callback: IAudioSessionNotification = SessionCreated {
                    signals: signals.clone(),
                    sender,
                }
                .into();
                manager.RegisterSessionNotification(&callback)?;
                Some(CreationRegistration { manager, callback })
            }
        } else {
            None
        };
        let mut watch = Self {
            _volume: volume,
            creation,
            incoming,
            sessions: BTreeMap::new(),
        };
        // Registration already has a Drop guard before GetCount, including failure paths.
        watch.seed_sessions(signals)?;
        Ok(watch)
    }
    fn seed_sessions(&mut self, signals: Arc<Signals>) -> Result<()> {
        if let Some(registration) = &self.creation {
            let enumerator = unsafe { registration.manager.GetSessionEnumerator()? };
            let count = unsafe { enumerator.GetCount()? };
            for index in 0..count {
                self.attach(unsafe { enumerator.GetSession(index)? }, signals.clone())?;
            }
        }
        Ok(())
    }
    fn attach(&mut self, control: IAudioSessionControl, signals: Arc<Signals>) -> Result<()> {
        unsafe {
            if control.GetState()? == AudioSessionStateExpired {
                return Ok(());
            }
            let extended: IAudioSessionControl2 = control.cast()?;
            let id = owned_string(extended.GetSessionInstanceIdentifier()?)?;
            if let std::collections::btree_map::Entry::Vacant(entry) = self.sessions.entry(id) {
                entry.insert(SessionRegistration::new(control, signals)?);
            }
        }
        Ok(())
    }
    fn refresh(&mut self, signals: Arc<Signals>, missed_changed: bool) -> Result<()> {
        // Snapshot enumeration does not replace the independently retained callback list.
        let new_sessions: Vec<_> = self.incoming.try_iter().collect();
        let mut first_error = None;
        for reference in new_sessions {
            if let Err(error) = reference
                .resolve()
                .and_then(|s| self.attach(s, signals.clone()))
            {
                signals.missed.fetch_add(1, Ordering::Relaxed);
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        self.sessions
            .retain(|_, session| !session.expired.load(Ordering::Acquire));
        if missed_changed || first_error.is_some() {
            self.seed_sessions(signals)?;
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }
}

/// Endpoint/session/volume registrations are rebuilt after device changes.
/// All reads and unregisters happen on this owning thread, never in callbacks.
pub struct AudioMonitor {
    enumerator: IMMDeviceEnumerator,
    callback: IMMNotificationClient,
    watches: BTreeMap<String, EndpointWatch>,
    signals: Arc<Signals>,
    device_revision_seen: u64,
    retry_at: Option<Instant>,
    _apartment: Apartment,
}
impl AudioMonitor {
    pub fn new() -> Result<Self> {
        let apartment = Apartment::new()?;
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
        let signals = Arc::new(Signals::default());
        signals.dirty.store(true, Ordering::Release);
        let callback: IMMNotificationClient = DeviceNotifications(signals.clone()).into();
        unsafe {
            enumerator.RegisterEndpointNotificationCallback(&callback)?;
        }
        Ok(Self {
            enumerator,
            callback,
            watches: BTreeMap::new(),
            signals,
            device_revision_seen: 0,
            retry_at: None,
            _apartment: apartment,
        })
    }
    pub fn take_changed(&self) -> bool {
        self.signals.dirty.swap(false, Ordering::AcqRel)
            || self.retry_at.is_some_and(|t| Instant::now() >= t)
    }
    pub fn snapshot(&mut self) -> Result<Snapshot> {
        let mut snapshot = match read_snapshot(&self.enumerator) {
            Ok(value) => value,
            Err(error) => {
                self.retry_at = Some(Instant::now() + Duration::from_secs(2));
                return Err(error);
            }
        };
        let revision = self.signals.devices.load(Ordering::Acquire);
        if revision != self.device_revision_seen {
            self.watches.clear();
            self.device_revision_seen = revision;
        }
        let active: BTreeSet<_> = snapshot.endpoints.iter().map(|e| e.id.as_str()).collect();
        self.watches.retain(|id, _| active.contains(id.as_str()));
        self.retry_at = None;
        let missed = self.signals.missed.load(Ordering::Acquire);
        for endpoint in &snapshot.endpoints {
            if !self.watches.contains_key(&endpoint.id) {
                let registration = unsafe {
                    self.enumerator
                        .GetDevice(&windows::core::HSTRING::from(endpoint.id.as_str()))
                }
                .and_then(|device| {
                    EndpointWatch::new(&device, endpoint.flow == "render", self.signals.clone())
                });
                match registration {
                    Ok(watch) => {
                        self.watches.insert(endpoint.id.clone(), watch);
                    }
                    Err(error) => {
                        snapshot
                            .warnings
                            .push(format!("monitor subscribe {}: {error}", endpoint.id));
                        self.retry_at = Some(Instant::now() + Duration::from_secs(2));
                    }
                }
            }
            if let Some(watch) = self.watches.get_mut(&endpoint.id) {
                if let Err(error) = watch.refresh(self.signals.clone(), missed > 0) {
                    snapshot
                        .warnings
                        .push(format!("monitor sessions {}: {error}", endpoint.id));
                    self.retry_at = Some(Instant::now() + Duration::from_secs(2));
                }
                for (id, session) in &watch.sessions {
                    match read_session(&session.control, &endpoint.id) {
                        Ok(value) => {
                            if let Some(existing) = snapshot
                                .sessions
                                .iter_mut()
                                .find(|s| s.endpoint_id == endpoint.id && s.instance_id == *id)
                            {
                                *existing = value;
                            } else {
                                snapshot.sessions.push(value);
                            }
                        }
                        Err(error) => {
                            snapshot
                                .warnings
                                .push(format!("monitored session {id}: {error}"));
                            self.retry_at = Some(Instant::now() + Duration::from_secs(2));
                        }
                    }
                }
            }
        }
        if missed > 0 {
            snapshot.warnings.push(format!("{missed} session events could not be retained; enumeration fallback may be incomplete"));
        }
        snapshot.monitor = Some(self.diagnostics());
        Ok(snapshot)
    }
    pub fn diagnostics(&self) -> MonitorDiagnostics {
        MonitorDiagnostics {
            endpoint_subscriptions: self.watches.len(),
            session_subscriptions: self.watches.values().map(|w| w.sessions.len()).sum(),
            device_events: self.signals.devices.load(Ordering::Acquire),
            session_created_events: self.signals.created.load(Ordering::Acquire),
            session_change_events: self.signals.sessions.load(Ordering::Acquire),
            endpoint_volume_events: self.signals.volumes.load(Ordering::Acquire),
            missed_session_events: self.signals.missed.load(Ordering::Acquire),
        }
    }
}
impl Drop for AudioMonitor {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .enumerator
                .UnregisterEndpointNotificationCallback(&self.callback);
        }
        self.watches.clear();
    }
}
