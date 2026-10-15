//! Read native-size icon resources as data, never execute the target program.
use super::{ExecutableIcon, crop_transparent_padding};
use std::io::Cursor;
use windows::{
    Win32::{
        Foundation::{FreeLibrary, HMODULE},
        System::LibraryLoader::{
            EnumResourceNamesW, FindResourceW, LOAD_LIBRARY_AS_DATAFILE,
            LOAD_LIBRARY_AS_IMAGE_RESOURCE, LoadLibraryExW, LoadResource, LockResource,
            SizeofResource,
        },
        UI::WindowsAndMessaging::{RT_GROUP_ICON, RT_ICON},
    },
    core::PCWSTR,
};

struct ResourceModule(HMODULE);
impl Drop for ResourceModule {
    fn drop(&mut self) {
        unsafe {
            let _ = FreeLibrary(self.0);
        }
    }
}

fn resource_bytes(module: HMODULE, name: PCWSTR, kind: PCWSTR) -> Option<Vec<u8>> {
    unsafe {
        let resource = FindResourceW(Some(module), name, kind);
        if resource.0.is_null() {
            return None;
        }
        let size = SizeofResource(Some(module), resource) as usize;
        if size == 0 || size > 4 * 1024 * 1024 {
            return None;
        }
        let loaded = LoadResource(Some(module), resource).ok()?;
        let data = LockResource(loaded).cast::<u8>();
        if data.is_null() {
            return None;
        }
        Some(std::slice::from_raw_parts(data, size).to_vec())
    }
}

unsafe extern "system" fn first_group(
    module: HMODULE,
    kind: PCWSTR,
    name: PCWSTR,
    state: isize,
) -> windows::core::BOOL {
    if let Some(data) = resource_bytes(module, name, kind)
        && data.len() >= 6
        && data[0..4] == [0, 0, 1, 0]
    {
        unsafe {
            *(state as *mut Option<Vec<u8>>) = Some(data);
        }
        return false.into();
    }
    true.into()
}

pub(super) fn best_native_icon(
    candidates: impl IntoIterator<Item = ExecutableIcon>,
) -> Option<ExecutableIcon> {
    candidates
        .into_iter()
        .filter(|icon| {
            icon.rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] > 8)
        })
        .map(crop_transparent_padding)
        .max_by_key(|icon| u64::from(icon.width) * u64::from(icon.height))
}

pub(super) fn from_executable(path: &str) -> Option<ExecutableIcon> {
    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let module = ResourceModule(unsafe {
        LoadLibraryExW(
            PCWSTR(wide.as_ptr()),
            None,
            LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
        )
        .ok()?
    });
    let mut group: Option<Vec<u8>> = None;
    unsafe {
        let _ = EnumResourceNamesW(
            Some(module.0),
            RT_GROUP_ICON,
            Some(first_group),
            &mut group as *mut _ as isize,
        );
    }
    let group = group?;
    let count = u16::from_le_bytes([group[4], group[5]]) as usize;
    if count == 0 || count > 256 || group.len() < 6 + count * 14 {
        return None;
    }
    let candidates = group[6..6 + count * 14]
        .as_chunks::<14>()
        .0
        .iter()
        .filter_map(|entry| {
            let id = u16::from_le_bytes([entry[12], entry[13]]);
            let raw = resource_bytes(module.0, PCWSTR(id as usize as *const u16), RT_ICON)?;
            // Reconstruct a one-image ICO directory around the unscaled resource.
            let mut ico = vec![0, 0, 1, 0, 1, 0];
            ico.extend_from_slice(&entry[..8]);
            ico.extend_from_slice(&(raw.len() as u32).to_le_bytes());
            ico.extend_from_slice(&22u32.to_le_bytes());
            ico.extend_from_slice(&raw);
            let mut reader =
                image::ImageReader::with_format(Cursor::new(ico), image::ImageFormat::Ico);
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(512);
            limits.max_image_height = Some(512);
            limits.max_alloc = Some(4 * 1024 * 1024);
            reader.limits(limits);
            reader.decode().ok().map(|decoded| {
                let rgba = decoded.into_rgba8();
                ExecutableIcon {
                    width: rgba.width(),
                    height: rgba.height(),
                    rgba: rgba.into_raw(),
                }
            })
        });
    best_native_icon(candidates)
}
