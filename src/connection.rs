// src/connection.rs

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub container_name: Option<String>,
    #[serde(default)]
    pub is_symlink: bool,

    #[serde(with = "chrono::serde::ts_seconds")]
    pub created_at: chrono::DateTime<chrono::Utc>, // Use Utc instead of Local
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub connections: Vec<Connection>,
}

impl ConnectionConfig {
    const CONFIG_FILE: &'static str = "server_connections.json";

    pub fn load() -> color_eyre::Result<Self> {
        let config_path = PathBuf::from(Self::CONFIG_FILE);
        if config_path.exists() {
            let content = fs::read_to_string(config_path)?;
            Ok(serde_json::from_str(&content)?)
        } else {
            Ok(Self {
                connections: Vec::new(),
            })
        }
    }

    pub fn save(&self) -> color_eyre::Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        fs::write(Self::CONFIG_FILE, content)?;
        Ok(())
    }

    pub fn add_connection(
        &mut self,
        name: String,
        path: PathBuf,
        container_name: Option<String>,
    ) -> color_eyre::Result<()> {
        // Validate path exists
        if !path.exists() {
            return Err(color_eyre::eyre::eyre!(
                "Path does not exist: {}",
                path.display()
            ));
        }

        // Check if path is a directory
        if !path.is_dir() {
            return Err(color_eyre::eyre::eyre!(
                "Path is not a directory: {}",
                path.display()
            ));
        }

        // Check for duplicate names
        if self.connections.iter().any(|c| c.name == name) {
            return Err(color_eyre::eyre::eyre!(
                "Connection name already exists: {}",
                name
            ));
        }

        // Detect if it's a symlink
        let is_symlink = path.is_symlink();

        self.connections.push(Connection {
            name,
            path,
            container_name: container_name
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            is_symlink,
            created_at: chrono::Utc::now(), // Use Utc::now()
        });

        self.save()?;
        Ok(())
    }

    pub fn remove_connection(&mut self, index: usize) -> color_eyre::Result<()> {
        if index < self.connections.len() {
            self.connections.remove(index);
            self.save()?;
        }
        Ok(())
    }

    pub fn get_server_paths(&self) -> Vec<PathBuf> {
        self.connections.iter().map(|c| c.path.clone()).collect()
    }
}
