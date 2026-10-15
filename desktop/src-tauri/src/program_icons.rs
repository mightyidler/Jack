//! Encode the same native icon pixels used by Bald; no program execution.
pub(crate) use crate::icon_cache::IconCache;
use base64::{Engine, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageFormat, RgbaImage};
use std::collections::BTreeSet;
use std::io::Cursor;
pub fn extract_png(path: &str) -> Result<Vec<u8>, String> {
    let icon = crate::bald_program_icons::executable_icon(path).ok_or("No executable icon")?;
    let image =
        RgbaImage::from_raw(icon.width, icon.height, icon.rgba).ok_or("Invalid icon pixels")?;
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok(bytes.into_inner())
}
pub fn data_url(path: &str) -> Option<String> {
    extract_png(path)
        .ok()
        .map(|bytes| format!("data:image/png;base64,{}", STANDARD.encode(bytes)))
}
pub fn verify(report: &std::path::Path) -> Result<(), String> {
    let mut monitor = jack_windows_audio::AudioMonitor::new().map_err(|error| error.to_string())?;
    let snapshot = monitor.snapshot().map_err(|error| error.to_string())?;
    let mut paths: BTreeSet<_> = snapshot
        .sessions
        .iter()
        .filter_map(|session| session.executable_path.clone())
        .collect();
    paths.insert(
        std::env::current_exe()
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .into(),
    );
    let folder = report
        .parent()
        .ok_or("Icon report directory required")?
        .join("program-icons");
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let mut results = Vec::new();
    let mut fixture = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        let result = extract_png(path);
        let name = std::path::Path::new(path)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        match result {
            Ok(bytes) => {
                let image = folder.join(format!("{index}.png"));
                std::fs::write(&image, &bytes).map_err(|error| error.to_string())?;
                results.push(serde_json::json!({"name":name,"path":path,"image":image,"bytes":bytes.len(),"passed":true}));
                fixture.push(serde_json::json!({"name":name,"stable_id":format!("path:{}",path.to_lowercase()),"icon_data_url":format!("data:image/png;base64,{}",STANDARD.encode(bytes))}));
            }
            Err(error) => results
                .push(serde_json::json!({"name":name,"path":path,"passed":false,"error":error})),
        }
    }
    std::fs::write(
        folder.join("fixture.json"),
        serde_json::to_vec(&fixture).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    std::fs::write(report,serde_json::to_vec_pretty(&serde_json::json!({"scope":"Read-only actual Windows Shell icon extraction; no native drag or audio claim","results":results})).map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
    Ok(())
}
