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
    let middle = bytes.len() / 2;
    bytes[middle] ^= 1;
    if key.verify(&bytes, &signature, true).is_ok() {
        return Err("Tampered installer was accepted".into());
    }
    println!("Installer signature verified with the app's public key; modified bytes rejected.");
    Ok(())
}
