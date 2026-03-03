use std::{
    fs,
    net::{TcpStream, UdpSocket},
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
    pub installed_resource_packs: Vec<InstalledPack>,
    pub installed_behavior_packs: Vec<InstalledPack>,
    pub port: Option<u16>,
    pub server_type: ServerType,
    pub container_name: Option<String>,
}

/// A pack that exists on disk (may or may not be enabled in the JSON).
#[derive(Debug, Clone)]
pub struct InstalledPack {
    pub uuid: String,
    pub name: String,
    pub version: Vec<u32>,
    pub enabled: bool,
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

// Minimal manifest structs for reading installed packs from disk.
#[derive(Deserialize)]
struct DiskManifestHeader {
    uuid: String,
    version: Vec<u32>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct DiskManifest {
    header: DiskManifestHeader,
}

impl ServerInstance {
    pub fn from_path(path: &Path, custom_name: Option<&str>) -> Self {
        let name = custom_name
            .map(|s| s.to_string())
            .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "Unknown".to_string());

        let resource_packs = load_pack_list(&path.join("resource_packs.json"));
        let behavior_packs = load_pack_list(&path.join("behavior_packs.json"));

        let installed_resource_packs =
            discover_installed_packs(&path.join("resource_packs"), &resource_packs);
        let installed_behavior_packs =
            discover_installed_packs(&path.join("behavior_packs"), &behavior_packs);

        // Detect server type and port
        let (server_type, port) = detect_server_config(path);

        // Bedrock uses UDP; Java uses TCP
        let status = match &server_type {
            ServerType::Bedrock => {
                let udp_port = port.unwrap_or(19132);
                if is_udp_port_in_use(udp_port) {
                    ServerStatus::Running
                } else {
                    detect_server_process(path)
                }
            }
            _ => {
                if let Some(p) = port {
                    if is_port_in_use(p) {
                        ServerStatus::Running
                    } else {
                        ServerStatus::Stopped
                    }
                } else {
                    detect_server_process(path)
                }
            }
        };

        let container_name = if server_type == ServerType::Bedrock {
            find_docker_container(path)
        } else {
            None
        };

        ServerInstance {
            name,
            path: path.to_path_buf(),
            status,
            resource_packs,
            behavior_packs,
            installed_resource_packs,
            installed_behavior_packs,
            port,
            server_type,
            container_name,
        }
    }
}

/// Scan `pack_dir` for subdirectories containing a `manifest.json` and return
/// an `InstalledPack` for each, with `enabled` set based on `enabled_entries`.
fn discover_installed_packs(pack_dir: &Path, enabled_entries: &[PackEntry]) -> Vec<InstalledPack> {
    let Ok(entries) = fs::read_dir(pack_dir) else {
        return Vec::new();
    };

    let mut packs: Vec<InstalledPack> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let manifest_path = e.path().join("manifest.json");
            let content = fs::read_to_string(&manifest_path).ok()?;
            let m: DiskManifest = serde_json::from_str(&content).ok()?;
            let enabled = enabled_entries.iter().any(|ep| ep.pack_id == m.header.uuid);
            Some(InstalledPack {
                name: m.header.name.unwrap_or_else(|| m.header.uuid.clone()),
                enabled,
                uuid: m.header.uuid,
                version: m.header.version,
            })
        })
        .collect();

    packs.sort_by(|a, b| a.name.cmp(&b.name));
    packs
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

/// Check if a TCP port is in use by trying to connect to it (Java servers)
fn is_port_in_use(port: u16) -> bool {
    TcpStream::connect_timeout(
        &format!("127.0.0.1:{}", port).parse().unwrap(),
        Duration::from_millis(100),
    )
    .is_ok()
}

/// Check if a UDP port is in use by attempting to bind it (Bedrock servers)
fn is_udp_port_in_use(port: u16) -> bool {
    UdpSocket::bind(("0.0.0.0", port)).is_err()
}

/// Find the Docker container that has `server_path` mounted as a volume
fn find_docker_container(server_path: &Path) -> Option<String> {
    let ids_out = Command::new("docker").args(["ps", "-q"]).output().ok()?;
    if !ids_out.status.success() || ids_out.stdout.is_empty() {
        return None;
    }
    let path_str = server_path.to_string_lossy();
    for id in String::from_utf8_lossy(&ids_out.stdout).lines() {
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        let mounts = Command::new("docker")
            .args(["inspect", "--format", "{{range .Mounts}}{{.Source}} {{end}}", id])
            .output()
            .ok()?;
        let mounts_str = String::from_utf8_lossy(&mounts.stdout);
        if mounts_str.contains(path_str.as_ref()) {
            let name_out = Command::new("docker")
                .args(["inspect", "--format", "{{.Name}}", id])
                .output()
                .ok()?;
            let name = String::from_utf8_lossy(&name_out.stdout)
                .trim()
                .trim_start_matches('/')
                .to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

/// Fetch the last `tail` lines from a Docker container's logs
pub fn read_docker_logs(container: &str, tail: usize) -> Vec<String> {
    let result = Command::new("docker")
        .args(["logs", "--tail", &tail.to_string(), container])
        .output();
    match result {
        Ok(o) => {
            // BDS writes to stderr inside the itzg container; capture both
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            let combined = match (stdout.is_empty(), stderr.is_empty()) {
                (true, _) => stderr.into_owned(),
                (_, true) => stdout.into_owned(),
                _ => format!("{}{}", stdout, stderr),
            };
            combined.lines().map(|l| l.to_string()).collect()
        }
        Err(e) => vec![format!("docker logs error: {e}")],
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
