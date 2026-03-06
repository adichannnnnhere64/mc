use serde::{Deserialize, Serialize};
use std::fmt; // Add Serialize

#[derive(Debug, Deserialize, Serialize)] // Added Serialize
pub struct Manifest {
    pub format_version: u32,
    pub header: ManifestHeader,
    #[serde(default)]
    pub modules: Vec<ManifestModule>,
}

#[derive(Debug, Deserialize, Serialize)] // Added Serialize
pub struct ManifestHeader {
    pub uuid: String,
    pub version: Vec<u32>,
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)] // Added Serialize
pub struct ManifestModule {
    #[serde(rename = "type")]
    pub module_type: String,
    pub uuid: String,

    pub version: Vec<u32>,
}

#[derive(Debug)]

pub enum PackType {
    Resources,
    Behavior,
    Unknown,
}

impl fmt::Display for PackType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackType::Resources => write!(f, "resource pack"),
            PackType::Behavior => write!(f, "behavior pack"),
            PackType::Unknown => write!(f, "unknown pack"),
        }
    }
}

impl Manifest {
    pub fn pack_type(&self) -> PackType {
        for module in &self.modules {
            match module.module_type.to_ascii_lowercase().as_str() {
                "resources" => return PackType::Resources,
                "data" | "client_data" | "server_data" | "javascript" | "script" | "scripting" => {
                    return PackType::Behavior;
                }
                _ => {}
            }
        }
        PackType::Unknown
    }
}
