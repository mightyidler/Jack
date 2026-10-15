//! Read-only package verification. Does not start Jack or run an installer.
use base64::{Engine, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file = std::env::args().nth(1).ok_or("Installer path required")?;
    let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json"))?;
    let key = STANDARD.decode(
        config["plugins"]["updater"]["pubkey"]
            .as_str()
            .ok_or("Public key missing")?,
    )?;
    let key = PublicKey::decode(std::str::from_utf8(&key)?)?;
    let signature = STANDARD.decode(std::fs::read_to_string(format!("{file}.sig"))?.trim())?;
    let signature = Signature::decode(std::str::from_utf8(&signature)?)?;
    let mut bytes = std::fs::read(file)?;
    key.verify(&bytes, &signature, true)?;
    if let Some(manifest) = std::env::args().nth(2) {
        let release: tauri_plugin_updater::RemoteRelease =
            serde_json::from_slice(&std::fs::read(manifest)?)?;
        if release.version.to_string() != config["version"].as_str().ok_or("Version missing")? {
            return Err("Manifest version differs from the app".into());
        }
        for target in ["windows-x86_64", "windows-x86_64-nsis"] {
            let manifest_signature = STANDARD.decode(release.signature(target)?.trim())?;
            let manifest_signature = Signature::decode(std::str::from_utf8(&manifest_signature)?)?;
            key.verify(&bytes, &manifest_signature, true)?;
            if release.download_url(target)?.scheme() != "https" {
                return Err("Updater download must use HTTPS".into());
            }
        }
        for tag in ["v0.1.4", "v0.1.5"] {
            let output = std::process::Command::new("git")
                .args(["show", &format!("{tag}:desktop/src-tauri/tauri.conf.json")])
                .output()?;
            if !output.status.success() {
                return Err(format!("Cannot read installed version config: {tag}").into());
            }
            let previous: serde_json::Value = serde_json::from_slice(&output.stdout)?;
            let previous_version = previous["version"].as_str().ok_or("Old version missing")?;
            if release.version.cmp(&previous_version.parse()?) != std::cmp::Ordering::Greater {
                return Err(format!("Update must be newer than {tag}").into());
            }
            let previous_key = STANDARD.decode(
                previous["plugins"]["updater"]["pubkey"]
                    .as_str()
                    .ok_or("Old public key missing")?,
            )?;
            let previous_key = PublicKey::decode(std::str::from_utf8(&previous_key)?)?;
            previous_key.verify(&bytes, &signature, true)?;
            println!(
                "{tag}: newer update recognized; installer accepted by the installed public key."
            );
        }
        println!("Updater plugin parsed the manifest and both Windows targets successfully.");
    }
    let middle = bytes.len() / 2;
    bytes[middle] ^= 1;
    if key.verify(&bytes, &signature, true).is_ok() {
        return Err("Tampered installer was accepted".into());
    }
    println!("Installer signature verified with the app's public key; modified bytes rejected.");
    Ok(())
}
