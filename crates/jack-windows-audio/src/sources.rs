//! Bind actual render endpoints to UUID-addressed Jack PnP devices through the
//! connected KS adapter interface. Friendly names never establish ownership.
use crate::{
    platform::{Apartment, Property, owned_string},
    virtual_devices::{self, GroupDevice},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, error::Error, fs::File, io::Read};
use uuid::Uuid;
use windows::{
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_InstanceId,
        Devices::{
            DeviceAndDriverInstallation::*,
            Properties::{DEVPKEY_Device_InstanceId, DEVPROP_TYPE_STRING},
        },
        Media::Audio::*,
        System::{
            Com::{
                CLSCTX_ALL, CoCreateInstance, STGM_READ,
                StructuredStorage::PropVariantToStringAlloc,
            },
            Registry::*,
        },
    },
    core::{HSTRING, w},
};

type SourceResult<T> = Result<T, Box<dyn Error>>;
// Reviewed, test-signed Group package. A new package requires an explicit pin
// update; no arbitrary path/hash from user settings can establish provenance.
const DRIVER_SHA256: &str = "08841F2ACEA9A518199A64463C5DF56335E8D27C33B6A40657032303AEF717BE";

#[derive(Debug, Serialize)]
pub struct RenderLink {
    pub endpoint_id: String,
    pub adapter_interface: String,
    pub adapter_instance_id: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct SourceBinding {
    pub group: Uuid,
    pub endpoint_id: String,
    pub device_instance_id: String,
}
#[derive(Debug, Serialize)]
pub struct DriverProvenance {
    pub path: String,
    pub sha256: String,
    pub matches_reviewed_binary: bool,
}
#[derive(Debug, Serialize)]
pub struct SourceInventory {
    pub devices: Vec<GroupDevice>,
    pub render_links: Vec<RenderLink>,
    pub sources: Vec<SourceBinding>,
    pub driver: Option<DriverProvenance>,
    pub warnings: Vec<String>,
}

fn adapter_instance(enumerator: &IMMDeviceEnumerator, interface: &str) -> SourceResult<String> {
    // This is a documented PnP interface property; don't parse its symbolic link.
    let mut bytes = [0u8; 4096];
    let mut size = bytes.len() as u32;
    let mut kind = Default::default();
    let result = unsafe {
        CM_Get_Device_Interface_PropertyW(
            &HSTRING::from(interface),
            &DEVPKEY_Device_InstanceId,
            &mut kind,
            Some(bytes.as_mut_ptr()),
            &mut size,
            0,
        )
    };
    if result == CR_SUCCESS
        && kind == DEVPROP_TYPE_STRING
        && size >= 2
        && size as usize <= bytes.len()
        && size.is_multiple_of(2)
    {
        let wide: Vec<_> = bytes[..size as usize]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        let end = wide
            .iter()
            .position(|c| *c == 0)
            .ok_or("unterminated adapter instance ID")?;
        return Ok(String::from_utf16(&wide[..end])?);
    }
    // Core Audio also accepts the connected adapter's device ID directly.
    let device = unsafe { enumerator.GetDevice(&HSTRING::from(interface))? };
    let properties = unsafe { device.OpenPropertyStore(STGM_READ)? };
    let property = Property(unsafe { properties.GetValue(&PKEY_Device_InstanceId)? });
    Ok(owned_string(unsafe {
        PropVariantToStringAlloc(&property.0)?
    })?)
}

fn provenance() -> SourceResult<DriverProvenance> {
    let mut bytes = [0u8; 65536];
    let mut size = bytes.len() as u32;
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SYSTEM\\CurrentControlSet\\Services\\JackAudio"),
            w!("ImagePath"),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            None,
            Some(bytes.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .ok()?;
    }
    if size as usize > bytes.len() || !size.is_multiple_of(2) {
        return Err("invalid driver service path".into());
    }
    let wide: Vec<_> = bytes[..size as usize]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    let end = wide
        .iter()
        .position(|c| *c == 0)
        .ok_or("unterminated driver service path")?;
    let raw = String::from_utf16(&wide[..end])?;
    let path = if raw.to_ascii_lowercase().starts_with("\\systemroot\\") {
        format!("{}\\{}", std::env::var("SystemRoot")?, &raw[12..])
    } else {
        raw.strip_prefix("\\??\\").unwrap_or(&raw).to_string()
    };
    if !std::path::Path::new(&path).is_absolute() {
        return Err("driver service path must be absolute".into());
    }
    let mut file = File::open(&path)?;
    let mut hash = Sha256::new();
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    let sha256 = format!("{:X}", hash.finalize());
    Ok(DriverProvenance {
        path,
        matches_reviewed_binary: sha256 == DRIVER_SHA256,
        sha256,
    })
}

/// Control-thread discovery only. Missing, foreign, ambiguous, inactive, or
/// changed drivers produce no verified bindings. Does not create devices/routes.
pub fn discover() -> SourceResult<SourceInventory> {
    let _apartment = Apartment::new()?;
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    let devices = virtual_devices::list()?;
    let mut result = SourceInventory {
        devices,
        render_links: vec![],
        sources: vec![],
        driver: None,
        warnings: vec![],
    };
    let endpoints = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)? };
    for index in 0..unsafe { endpoints.GetCount()? } {
        let device = unsafe { endpoints.Item(index)? };
        let endpoint_id = owned_string(unsafe { device.GetId()? })?;
        let read_link = || -> SourceResult<RenderLink> {
            let topology: IDeviceTopology = unsafe { device.Activate(CLSCTX_ALL, None)? };
            if unsafe { topology.GetConnectorCount()? } != 1 {
                return Err("render endpoint has ambiguous topology connectors".into());
            }
            let connector = unsafe { topology.GetConnector(0)? };
            let adapter_interface = owned_string(unsafe { connector.GetDeviceIdConnectedTo()? })?;
            let adapter_instance_id = adapter_instance(&enumerator, &adapter_interface)?;
            Ok(RenderLink {
                endpoint_id: endpoint_id.clone(),
                adapter_interface,
                adapter_instance_id,
            })
        };
        match read_link() {
            Ok(link) => result.render_links.push(link),
            Err(error) => result.warnings.push(format!("{endpoint_id}: {error}")),
        }
    }
    if result.devices.is_empty() {
        return Ok(result);
    }
    match provenance() {
        Ok(driver) => result.driver = Some(driver),
        Err(error) => result
            .warnings
            .push(format!("Jack driver provenance: {error}")),
    }
    if !result
        .driver
        .as_ref()
        .is_some_and(|d| d.matches_reviewed_binary)
    {
        return Ok(result);
    }
    let mut used = BTreeSet::new();
    for device in &result.devices {
        if !device.device_started {
            continue;
        }
        let links: Vec<_> = result
            .render_links
            .iter()
            .filter(|l| {
                l.adapter_instance_id
                    .eq_ignore_ascii_case(&device.instance_id)
            })
            .collect();
        if links.len() != 1 {
            result.warnings.push(format!(
                "group {} has {} active render endpoints; expected one",
                device.group,
                links.len()
            ));
            continue;
        }
        let link = links[0];
        if !used.insert(link.endpoint_id.clone()) {
            return Err("Jack groups share an endpoint; refusing source bindings".into());
        }
        result.sources.push(SourceBinding {
            group: device.group,
            endpoint_id: link.endpoint_id.clone(),
            device_instance_id: device.instance_id.clone(),
        });
    }
    Ok(result)
}

pub fn verify_plan(plan: &jack_core::runtime::EnginePlan) -> Result<(), String> {
    let inventory = discover().map_err(|e| e.to_string())?;
    if plan.outputs.iter().any(|output| {
        inventory
            .sources
            .iter()
            .any(|source| source.endpoint_id == output.endpoint)
    }) {
        return Err("Jack virtual sources cannot be transport outputs".into());
    }
    for group in &plan.groups {
        if !inventory
            .sources
            .iter()
            .any(|source| source.group == group.id && source.endpoint_id == group.source)
        {
            return Err(format!(
                "group {} does not own an active, verified Jack render source",
                group.id
            ));
        }
    }
    Ok(())
}
