//! Secure credential storage using XOR encryption

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

const TOKEN_FILE_NAME: &str = "hf_token.enc";

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Crypto error: {0}")]
    Crypto(String),
}

pub fn get_token_file_path() -> Result<PathBuf, StorageError> {
    let app_data_dir = dirs::data_local_dir()
        .ok_or_else(|| StorageError::Crypto("Cannot get app data directory".to_string()))?
        .join("LlamaUI");
    fs::create_dir_all(&app_data_dir)?;
    Ok(app_data_dir.join(TOKEN_FILE_NAME))
}

pub fn save_token_securely(token: &str) -> Result<(), StorageError> {
    let encrypted = encrypt_data(token.as_bytes());
    let path = get_token_file_path()?;
    let mut file = fs::File::create(&path)?;
    file.write_all(b"LUAITE")?;
    file.write_all(&(encrypted.len() as u32).to_le_bytes())?;
    file.write_all(&encrypted)?;
    file.flush()?;
    Ok(())
}

pub fn load_token_securely() -> Result<Option<String>, StorageError> {
    let path = get_token_file_path()?;
    if !path.exists() { return Ok(None); }

    let mut file = fs::File::open(&path)?;
    let mut header = [0u8; 6];
    file.read_exact(&mut header)?;
    if &header != b"LUAITE" { return Err(StorageError::Crypto("Invalid format".into())); }

    let mut len_bytes = [0u8; 4];
    file.read_exact(&mut len_bytes)?;
    let mut encrypted = vec![0u8; u32::from_le_bytes(len_bytes) as usize];
    file.read_exact(&mut encrypted)?;

    String::from_utf8(decrypt_data(&encrypted))
        .map_err(|e| StorageError::Crypto(format!("Decode failed: {}", e)))
        .map(Some)
}

pub fn delete_token_securely() -> Result<(), StorageError> {
    let path = get_token_file_path()?;
    if path.exists() { fs::remove_file(&path)?; }
    Ok(())
}

fn encrypt_data(data: &[u8]) -> Vec<u8> { xor_encrypt(data, b"LlamaUI-Secret-Key-2026") }

fn decrypt_data(encrypted: &[u8]) -> Vec<u8> { xor_decrypt(encrypted, b"LlamaUI-Secret-Key-2026") }

fn xor_encrypt(data: &[u8], key: &[u8]) -> Vec<u8> {
    data.iter().enumerate().map(|(i, &byte)| byte ^ key[i % key.len()]).collect()
}

fn xor_decrypt(encrypted: &[u8], key: &[u8]) -> Vec<u8> {
    xor_encrypt(encrypted, key)
}