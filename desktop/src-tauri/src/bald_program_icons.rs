//! Program icon pipeline reused from the user's Bald project.
pub struct ExecutableIcon {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[path = "executable_icon_resources.rs"]
mod icon_resources;

pub fn executable_icon(path: &str) -> Option<ExecutableIcon> {
    icon_resources::from_executable(path)
        .or_else(|| high_resolution_executable_icon(path))
        .or_else(|| legacy_executable_icon(path))
}

fn crop_transparent_padding(icon: ExecutableIcon) -> ExecutableIcon {
    let mut left = icon.width;
    let mut top = icon.height;
    let mut right = 0;
    let mut bottom = 0;
    for y in 0..icon.height {
        for x in 0..icon.width {
            if icon.rgba[((y * icon.width + x) * 4 + 3) as usize] > 8 {
                left = left.min(x);
                top = top.min(y);
                right = right.max(x + 1);
                bottom = bottom.max(y + 1);
            }
        }
    }
    if left >= right
        || top >= bottom
        || (left == 0 && top == 0 && right == icon.width && bottom == icon.height)
    {
        return icon;
    }
    let width = right - left;
    let height = bottom - top;
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in top..bottom {
        let start = ((y * icon.width + left) * 4) as usize;
        rgba.extend_from_slice(&icon.rgba[start..start + (width * 4) as usize]);
    }
    ExecutableIcon {
        width,
        height,
        rgba,
    }
}

fn high_resolution_executable_icon(path: &str) -> Option<ExecutableIcon> {
    use std::mem::size_of;
    use windows::{
        Win32::{
            Foundation::SIZE,
            Graphics::Gdi::{
                BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS,
                DeleteDC, DeleteObject, GetDIBits, GetObjectW, HGDIOBJ,
            },
            System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
            UI::Shell::{
                IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK,
                SIIGBF_ICONONLY,
            },
        },
        core::PCWSTR,
    };

    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    struct Apartment(bool);
    impl Drop for Apartment {
        fn drop(&mut self) {
            if self.0 {
                unsafe {
                    CoUninitialize();
                }
            }
        }
    }
    unsafe {
        let _apartment = Apartment(CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok());
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).ok()?;
        let bitmap = factory
            .GetImage(
                SIZE { cx: 256, cy: 256 },
                SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK,
            )
            .ok()?;
        let mut details = BITMAP::default();
        if GetObjectW(
            HGDIOBJ(bitmap.0),
            size_of::<BITMAP>() as i32,
            Some((&mut details as *mut BITMAP).cast()),
        ) == 0
        {
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            return None;
        }
        let width = details.bmWidth.max(1) as u32;
        let height = details.bmHeight.max(1) as u32;
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = CreateCompatibleDC(None);
        if dc.0.is_null() {
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            return None;
        }
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            height,
            Some(rgba.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteDC(dc);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        if lines == 0 {
            return None;
        }
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        Some(crop_transparent_padding(ExecutableIcon {
            width,
            height,
            rgba,
        }))
    }
}

fn legacy_executable_icon(path: &str) -> Option<ExecutableIcon> {
    use std::{ffi::c_void, mem::size_of, ptr::copy_nonoverlapping};
    use windows::{
        Win32::{
            Graphics::Gdi::{
                BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection,
                DIB_RGB_COLORS, DeleteDC, DeleteObject, HGDIOBJ, SelectObject,
            },
            UI::{
                Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGetFileInfoW},
                WindowsAndMessaging::{DI_NORMAL, DestroyIcon, DrawIconEx},
            },
        },
        core::PCWSTR,
    };

    const SIZE: i32 = 32;
    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut info = SHFILEINFOW::default();
    let found = unsafe {
        SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            Default::default(),
            Some(&mut info),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    if found == 0 || info.hIcon.0.is_null() {
        return None;
    }

    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.0.is_null() {
            let _ = DestroyIcon(info.hIcon);
            return None;
        }
        let bitmap_info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: SIZE,
                biHeight: -SIZE,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(dc), &bitmap_info, DIB_RGB_COLORS, &mut bits, None, 0);
        let Ok(bitmap) = bitmap else {
            let _ = DeleteDC(dc);
            let _ = DestroyIcon(info.hIcon);
            return None;
        };
        let old = SelectObject(dc, HGDIOBJ(bitmap.0));
        let drawn = DrawIconEx(dc, 0, 0, info.hIcon, SIZE, SIZE, 0, None, DI_NORMAL).is_ok();
        let mut bgra = vec![0u8; (SIZE * SIZE * 4) as usize];
        if drawn && !bits.is_null() {
            copy_nonoverlapping(bits.cast::<u8>(), bgra.as_mut_ptr(), bgra.len());
        }
        SelectObject(dc, old);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(dc);
        let _ = DestroyIcon(info.hIcon);
        drawn.then(|| {
            for pixel in bgra.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
                if pixel[3] == 0 && (pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0) {
                    pixel[3] = 255;
                }
            }
            ExecutableIcon {
                width: SIZE as u32,
                height: SIZE as u32,
                rgba: bgra,
            }
        })
    }
}
