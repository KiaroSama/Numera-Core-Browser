use serde::Deserialize;
use std::{fs, path::Path};

#[derive(Deserialize)]
pub struct Lock {
    pub package: Vec<Package>,
}

#[derive(Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub source: Option<String>,
    pub checksum: Option<String>,
}

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub fn read_lock(path: &Path) -> Result<Lock> {
    Ok(toml::from_str(&fs::read_to_string(path)?)?)
}
