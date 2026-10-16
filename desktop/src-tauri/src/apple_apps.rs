//! Presentation for Apple's packaged apps, whose audio helpers share a name.
use std::path::Path;

pub struct AppleApp<'a> {
    root: &'a Path,
    pub display_name: &'static str,
}

pub fn from_executable(path: &str) -> Option<AppleApp<'_>> {
    let executable = Path::new(path);
    let root = executable.parent()?;
    if !root
        .parent()?
        .file_name()?
        .to_str()?
        .eq_ignore_ascii_case("WindowsApps")
    {
        return None;
    }
    // Match the complete package name and publisher, never a substring or the
    // helper's basename alone. The version/architecture can change on update.
    let package = root.file_name()?.to_str()?.to_ascii_lowercase();
    let fields: Vec<_> = package.split('_').collect();
    if fields.len() != 5 || fields[4] != "nzyj5cx40ttqa" {
        return None;
    }
    let (display_name, main_executable) = match fields[0] {
        "appleinc.applemusicwin" => ("Apple Music", "applemusic.exe"),
        "appleinc.appletvwin" => ("Apple TV", "appletv.exe"),
        _ => return None,
    };
    let name = executable.file_name()?.to_str()?.to_ascii_lowercase();
    if name != "amplibraryagent.exe" && name != main_executable {
        return None;
    }
    Some(AppleApp { root, display_name })
}

impl AppleApp<'_> {
    pub fn icon_png(&self) -> Option<Vec<u8>> {
        // Both packages declare Images/Square44x44Logo in AppxManifest.xml.
        // Prefer the full-color, unplated asset used for desktop app icons.
        [
            "Square44x44Logo.targetsize-256_altform-unplated.png",
            "Square44x44Logo.targetsize-256.png",
            "Square44x44Logo.scale-400.png",
            "Square44x44Logo.scale-200.png",
            "Square44x44Logo.scale-100.png",
            "Square44x44Logo.png",
        ]
        .into_iter()
        .find_map(|name| {
            let bytes = std::fs::read(self.root.join("Images").join(name)).ok()?;
            let image =
                image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?;
            (image.width() > 0 && image.height() > 0).then_some(bytes)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_helpers_resolve_to_their_own_app_across_versions_and_architectures() {
        for (package, name, executable) in [
            (
                "AppleInc.AppleMusicWin_1.2.3.4_x64__nzyj5cx40ttqa",
                "Apple Music",
                "AppleMusic.exe",
            ),
            (
                "AppleInc.AppleTVWin_2.3.4.5_arm64__nzyj5cx40ttqa",
                "Apple TV",
                "AppleTV.exe",
            ),
        ] {
            for executable in [executable, "AMPLibraryAgent.exe"] {
                let path = format!("C:/Program Files/WindowsApps/{package}/{executable}");
                assert_eq!(from_executable(&path).unwrap().display_name, name);
                assert_eq!(
                    from_executable(&path.to_uppercase()).unwrap().display_name,
                    name
                );
            }
        }
    }

    #[test]
    fn unrelated_helpers_and_packages_keep_their_existing_presentation() {
        for path in [
            "C:/Apps/AMPLibraryAgent.exe",
            "C:/WindowsApps/Other.App_1.0_x64__nzyj5cx40ttqa/AMPLibraryAgent.exe",
            "C:/WindowsApps/AppleInc.AppleMusicWinExtra_1.0_x64__nzyj5cx40ttqa/AMPLibraryAgent.exe",
            "C:/WindowsApps/AppleInc.AppleMusicWin_1.0_x64__other/AMPLibraryAgent.exe",
            "C:/WindowsApps/AppleInc.AppleTVWin_1.0_x64__nzyj5cx40ttqa/AppleMusic.exe",
            "C:/WindowsApps/AppleInc.AppleMusicWin_1.0_x64__nzyj5cx40ttqa/Other.exe",
        ] {
            assert!(from_executable(path).is_none(), "{path}");
        }
    }
}
