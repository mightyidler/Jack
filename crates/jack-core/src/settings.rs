use crate::{ModelError, Settings};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error("invalid primary settings and no valid backup; originals preserved")]
    Corrupt,
}

#[derive(Debug, PartialEq)]
pub enum LoadSource {
    New,
    Primary,
    Backup,
}
pub struct LoadedSettings {
    pub settings: Settings,
    pub source: LoadSource,
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("json.bak")
}
fn read_valid(path: &Path) -> Result<Settings, StoreError> {
    let value: Settings = serde_json::from_slice(&fs::read(path)?)?;
    value.validate()?;
    Ok(value)
}

pub fn load(path: &Path) -> Result<LoadedSettings, StoreError> {
    match read_valid(path) {
        Ok(settings) => Ok(LoadedSettings {
            settings,
            source: LoadSource::Primary,
        }),
        // Future schemas must not be downgraded through backup/default fallback.
        Err(StoreError::Model(ModelError::Schema(v))) => Err(ModelError::Schema(v).into()),
        Err(StoreError::Io(e)) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
        Err(error) => match read_valid(&backup_path(path)) {
            Ok(settings) => Ok(LoadedSettings {
                settings,
                source: LoadSource::Backup,
            }),
            Err(StoreError::Model(ModelError::Schema(v))) => Err(ModelError::Schema(v).into()),
            Err(StoreError::Io(e)) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
            Err(_)
                if matches!(error, StoreError::Io(ref e) if e.kind() == io::ErrorKind::NotFound)
                    && !backup_path(path).exists() =>
            {
                Ok(LoadedSettings {
                    settings: Settings::default(),
                    source: LoadSource::New,
                })
            }
            Err(_) => Err(StoreError::Corrupt),
        },
    }
}

/// Single-writer store. Sync a same-directory temp file, then atomically replace.
/// Never overwrite a corrupt primary or a newer schema; load/recovery is explicit.
pub fn save(path: &Path, settings: &Settings) -> Result<(), StoreError> {
    settings.validate()?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let previous = read_valid(path)?;
        write_atomic(&backup_path(path), &serde_json::to_vec_pretty(&previous)?)?;
    }
    write_atomic(path, &serde_json::to_vec_pretty(settings)?)?;
    Ok(())
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        replace(&temp, path)?;
        #[cfg(not(windows))]
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(windows)]
fn replace(source: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        },
        core::PCWSTR,
    };
    let from: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: both paths are live null-terminated UTF-16 buffers for the whole call.
    unsafe {
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(io::Error::other)
}
#[cfg(not(windows))]
fn replace(source: &Path, target: &Path) -> io::Result<()> {
    fs::rename(source, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn device_levels_survive_disk_roundtrip_and_legacy_settings_start_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut settings = Settings::default();
        // Old files contain no device_volumes; serde defaults must keep them loadable.
        let legacy = serde_json::to_value(&settings).unwrap();
        assert!(legacy["groups"][0].get("device_volumes").is_none());
        let loaded: Settings = serde_json::from_value(legacy).unwrap();
        assert!(loaded.groups.iter().all(|g| g.device_volumes.is_empty()));
        settings.groups[0]
            .device_volumes
            .insert("headphones-full-id".into(), 0.0);
        settings.groups[0]
            .device_volumes
            .insert("speakers-full-id".into(), 0.76);
        settings.groups[1]
            .device_volumes
            .insert("headphones-full-id".into(), 0.42);
        save(&path, &settings).unwrap();
        assert_eq!(load(&path).unwrap().settings, settings);
        for invalid in [-0.1, 1.1, f64::NAN, f64::INFINITY] {
            settings.groups[0]
                .device_volumes
                .insert("invalid".into(), invalid);
            assert!(save(&path, &settings).is_err());
        }
    }
    #[test]
    fn saves_overrides_and_recovers_backup_without_erasing_primary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(load(&path).unwrap().source, LoadSource::New);
        let mut s = Settings::default();
        s.assign_app("stable-app".into(), crate::MEDIA).unwrap();
        save(&path, &s).unwrap();
        assert_eq!(load(&path).unwrap().settings, s);
        s.groups[0].volume = 0.65;
        save(&path, &s).unwrap();
        fs::write(&path, "broken").unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.source, LoadSource::Backup);
        assert_eq!(
            loaded.settings.user_app_overrides["stable-app"],
            crate::MEDIA
        );
        assert!(save(&path, &s).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "broken");
    }
    #[test]
    fn future_schema_is_preserved_even_with_valid_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        save(&path, &Settings::default()).unwrap();
        save(&path, &Settings::default()).unwrap();
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value["schema_version"] = 99.into();
        fs::write(&path, value.to_string()).unwrap();
        assert!(matches!(
            load(&path),
            Err(StoreError::Model(ModelError::Schema(99)))
        ));
    }
}
