use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct ServerInstance {
    pub name: String,
    pub path: PathBuf,
    pub status: ServerStatus,
    pub resource_packs: Vec<PackEntry>,
    pub behavior_packs: Vec<PackEntry>,
}

#[derive(Debug, Clone)]
pub enum ServerStatus {
    Running,
    Stopped,
    Starting,
    Error(String),
}

impl ServerStatus {
    pub fn label(&self) -> &str {
        match self {
            Self::Running => "RUNNING",
            Self::Stopped => "STOPPED",
            Self::Starting => "STARTING",
            Self::Error(_) => "ERROR",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackEntry {
    pub pack_id: String,
    pub version: Vec<u32>,
}

/// Scan `base` directory for subdirectories and treat each as a server instance.
pub fn discover_servers(base: &Path) -> Vec<ServerInstance> {
    let Ok(entries) = fs::read_dir(base) else {
        return Vec::new();
    };

    let mut servers: Vec<ServerInstance> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let path = e.path();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let resource_packs = load_pack_list(&path.join("resource_packs.json"));
            let behavior_packs = load_pack_list(&path.join("behavior_packs.json"));
            ServerInstance {
                name,
                path,
                status: ServerStatus::Stopped,
                resource_packs,
                behavior_packs,
            }
        })
        .collect();

    servers.sort_by(|a, b| a.name.cmp(&b.name));
    servers
}

fn load_pack_list(path: &Path) -> Vec<PackEntry> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}
