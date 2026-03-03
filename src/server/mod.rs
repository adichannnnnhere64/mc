use std::{
    fs,
    net::TcpStream,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct ServerInstance {
    pub name: String,

    pub path: PathBuf,
    pub status: ServerStatus,
    pub resource_packs: Vec<PackEntry>,

    pub behavior_packs: Vec<PackEntry>,
    pub port: Option<u16>,
    pub server_type: ServerType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerType {

    Bedrock,

    Java,
    Unknown,
}

impl ServerType {
    pub fn as_str(&self) -> &str {

        match self {

            Self::Bedrock => "Bedrock",
            Self::Java => "Java",
            Self::Unknown => "Unknown",

        }
    }
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

impl ServerInstance {
    pub fn from_path(path: &Path, custom_name: Option<&str>) -> Self {
        let name = custom_name
            .map(|s| s.to_string())
            .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "Unknown".to_string());
        
        let resource_packs = load_pack_list(&path.join("resource_packs.json"));
        let behavior_packs = load_pack_list(&path.join("behavior_packs.json"));
        
        // Detect server type and port
        let (server_type, port) = detect_server_config(path);
        
        // Detect if server is running
        let status = if let Some(port) = port {

            if is_port_in_use(port) {
                ServerStatus::Running
            } else {
                ServerStatus::Stopped
            }
        } else {
            // If no port configured, try to detect via process
            detect_server_process(path)
        };
        
        ServerInstance {
            name,
            path: path.to_path_buf(),

            status,
            resource_packs,
            behavior_packs,
            port,
            server_type,
        }
    }
}

/// Detect server type and port from server.properties or bedrock server files
fn detect_server_config(server_path: &Path) -> (ServerType, Option<u16>) {
    // Check for Bedrock server files first
    let bedrock_props = server_path.join("server.properties");
    let bedrock_exe = server_path.join("bedrock_server");
    let bedrock_exe_windows = server_path.join("bedrock_server.exe");
    
    if bedrock_props.exists() || bedrock_exe.exists() || bedrock_exe_windows.exists() {
        // Read port from bedrock server.properties
        if let Some(port) = read_bedrock_port(&bedrock_props) {
            return (ServerType::Bedrock, Some(port));
        }
        return (ServerType::Bedrock, Some(19132)); // Default Bedrock port
    }

    
    // Check for Java server files
    let java_props = server_path.join("server.properties");
    let java_jar = server_path.join("server.jar");
    let paper_jar = server_path.join("paper.jar");

    let spigot_jar = server_path.join("spigot.jar");
    
    if java_props.exists() || java_jar.exists() || paper_jar.exists() || spigot_jar.exists() {

        // Read port from java server.properties
        if let Some(port) = read_java_port(&java_props) {
            return (ServerType::Java, Some(port));
        }
        return (ServerType::Java, Some(25565)); // Default Java port
    }
    
    (ServerType::Unknown, None)
}


/// Read port from Bedrock server.properties
fn read_bedrock_port(props_path: &Path) -> Option<u16> {
    if !props_path.exists() {
        return None;
    }
    
    let content = fs::read_to_string(props_path).ok()?;
    for line in content.lines() {
        if line.starts_with("server-port=") {
            if let Some(port_str) = line.split('=').nth(1) {
                if let Ok(port) = port_str.trim().parse::<u16>() {
                    return Some(port);
                }
            }
        }
    }
    None

}


/// Read port from Java server.properties
fn read_java_port(props_path: &Path) -> Option<u16> {
    if !props_path.exists() {

        return None;
    }
    
    let content = fs::read_to_string(props_path).ok()?;
    for line in content.lines() {
        if line.starts_with("server-port=") {
            if let Some(port_str) = line.split('=').nth(1) {
                if let Ok(port) = port_str.trim().parse::<u16>() {
                    return Some(port);
                }
            }
        }
    }
    None
}

/// Check if a port is in use by trying to connect to it
fn is_port_in_use(port: u16) -> bool {
    match TcpStream::connect_timeout(
        &format!("127.0.0.1:{}", port).parse().unwrap(),
        Duration::from_millis(100),
    ) {
        Ok(_) => true,  // Port is open and accepting connections
        Err(_) => false, // Port is not accepting connections
    }
}


/// Detect if server process is running
fn detect_server_process(server_path: &Path) -> ServerStatus {
    let server_name = server_path.file_name().unwrap_or_default().to_string_lossy();
    
    #[cfg(target_os = "linux")]
    {
        // Check for Bedrock server process
        if let Ok(output) = Command::new("pgrep")
            .args(["-f", "bedrock_server"])
            .output()
        {
            if output.status.success() {
                return ServerStatus::Running;
            }
        }
        
        // Check for Java server process
        if let Ok(output) = Command::new("pgrep")

            .args(["-f", "java.*server.jar"])
            .output()
        {
            if output.status.success() {
                return ServerStatus::Running;
            }
        }
    }

    
    #[cfg(target_os = "windows")]
    {
        // Check for Bedrock server process
        if let Ok(output) = Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq bedrock_server.exe", "/FO", "CSV"])
            .output()
        {
            let output_str = String::from_utf8_lossy(&output.stdout);
            if output_str.contains("bedrock_server.exe") {

                return ServerStatus::Running;
            }
        }
        
        // Check for Java server process
        if let Ok(output) = Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq java.exe", "/FO", "CSV"])
            .output()
        {
            let output_str = String::from_utf8_lossy(&output.stdout);
            if output_str.contains("java.exe") && output_str.contains(&*server_name) {
                return ServerStatus::Running;
            }
        }
    }
    
    #[cfg(target_os = "macos")]
    {
        // Check for Bedrock server process
        if let Ok(output) = Command::new("pgrep")
            .args(["-f", "bedrock_server"])

            .output()
        {
            if output.status.success() {
                return ServerStatus::Running;
            }
        }
        
        // Check for Java server process

        if let Ok(output) = Command::new("pgrep")
            .args(["-f", "java.*server.jar"])
            .output()
        {
            if output.status.success() {
                return ServerStatus::Running;
            }
        }
    }
    
    ServerStatus::Stopped
}

/// Scan `base` directory for subdirectories and treat each as a server instance.
pub fn discover_servers(base: &Path) -> Vec<ServerInstance> {

    let Ok(entries) = fs::read_dir(base) else {
        return Vec::new();
    };

    entries

        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| ServerInstance::from_path(&e.path(), None))
        .collect()
}

fn load_pack_list(path: &Path) -> Vec<PackEntry> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}
