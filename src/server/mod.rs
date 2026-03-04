use std::{
    fs,
    io::{Read, Write},
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
    pub pid: Option<u32>,
    pub ram_mb: Option<u64>,
    pub cpu_percent: Option<f32>,
    pub players_online: Option<u32>,
    pub players_max: Option<u32>,
    pub cpu_sample: Option<CpuSnapshot>,
}

#[derive(Debug, Clone)]
pub struct CpuSnapshot {
    pub process_jiffies: u64,
    pub total_jiffies: u64,
}

/// Input for a background status refresh (send to spawn_blocking).
#[derive(Debug, Clone)]
pub struct StatusRefreshInput {
    pub path: PathBuf,
    pub server_type: ServerType,
    pub port: Option<u16>,
    pub container_name: Option<String>,
    pub prev_cpu_sample: Option<CpuSnapshot>,
}

/// Output from a background status refresh.
#[derive(Debug, Clone)]
pub struct StatusUpdate {
    pub path: PathBuf,
    pub status: ServerStatus,
    pub pid: Option<u32>,
    pub ram_mb: Option<u64>,
    pub cpu_percent: Option<f32>,
    pub players_online: Option<u32>,
    pub players_max: Option<u32>,
    pub new_cpu_sample: Option<CpuSnapshot>,
}

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]

pub struct PackEntry {
    pub pack_id: String,
    pub version: Vec<u32>,
}

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
    // NEW: Refresh status without rebuilding the whole instance
    pub fn refresh_status(&mut self) {
        self.status = detect_server_status(&self.server_type, self.port, &self.path);

        if !matches!(self.status, ServerStatus::Running) {
            self.pid = None;
            self.ram_mb = None;
            self.cpu_percent = None;
            self.players_online = None;
            self.players_max = None;
            self.cpu_sample = None;
            return;
        }

        self.pid = if let Some(ref c) = self.container_name {
            find_pid_for_container(c)
        } else {
            find_process_pid(&self.server_type)
        };

        let (ram_mb, cpu_percent) = if let Some(container) = self.container_name.as_deref() {
            get_docker_stats(container)
                .map(|(ram, cpu)| (Some(ram), Some(cpu)))
                .unwrap_or((None, None))
        } else {
            let ram = self.pid.and_then(get_process_ram_mb);
            let cpu = self.pid.and_then(|pid| {
                let current = read_cpu_snapshot(pid)?;
                let cpu = self
                    .cpu_sample
                    .as_ref()
                    .and_then(|prev| calc_cpu_percent(prev, &current));
                self.cpu_sample = Some(current);
                cpu
            });
            (ram, cpu)
        };

        self.ram_mb = ram_mb;
        self.cpu_percent = cpu_percent;

        let players = match self.server_type {
            ServerType::Java => self.port.and_then(query_java_player_count),
            ServerType::Bedrock => self.port.and_then(query_bedrock_player_count),
            ServerType::Unknown => None,
        };
        self.players_online = players.map(|(online, _)| online);
        self.players_max = players.map(|(_, max)| max);
    }

    /// Apply a `StatusUpdate` returned from a background `compute_status_update` call.
    pub fn apply_status_update(&mut self, update: StatusUpdate) {
        self.status = update.status;
        self.pid = update.pid;
        self.ram_mb = update.ram_mb;
        self.cpu_percent = update.cpu_percent;
        self.players_online = update.players_online;
        self.players_max = update.players_max;
        self.cpu_sample = update.new_cpu_sample;
    }

    pub fn from_path(path: &Path, custom_name: Option<&str>) -> Self {
        let name = custom_name
            .map(|s| s.to_string())
            .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "Unknown".to_string());

        let worlds_root = detect_worlds_root(path);
        let world_dir = read_level_name(&path)
            .map(|n| worlds_root.join(n))
            .unwrap_or_else(|| worlds_root.join("Bedrock level"));

        let resource_packs = load_pack_list(&world_dir.join("world_resource_packs.json"));
        let behavior_packs = load_pack_list(&world_dir.join("world_behavior_packs.json"));

        let installed_resource_packs =
            discover_installed_packs(&world_dir.join("resource_packs"), &resource_packs);
        let installed_behavior_packs =
            discover_installed_packs(&world_dir.join("behavior_packs"), &behavior_packs);

        let (server_type, port) = detect_server_config(path);
        // Use the new helper for status detection
        let status = detect_server_status(&server_type, port, path);
        let container_name = if server_type == ServerType::Bedrock {
            find_docker_container(path)
        } else {
            None
        };

        let pid = if let Some(ref c) = container_name {
            find_pid_for_container(c)
        } else if matches!(status, ServerStatus::Running) {
            find_process_pid(&server_type)
        } else {
            None
        };

        let ram_mb = pid.and_then(get_process_ram_mb).or_else(|| {
            container_name
                .as_deref()
                .and_then(|c| get_docker_stats(c).map(|(ram, _)| ram))
        });

        let players = if matches!(status, ServerStatus::Running) {
            match server_type {
                ServerType::Java => port.and_then(query_java_player_count),
                ServerType::Bedrock => port.and_then(query_bedrock_player_count),
                ServerType::Unknown => None,
            }
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
            pid,
            ram_mb,
            cpu_percent: None,
            players_online: players.map(|(online, _)| online),
            players_max: players.map(|(_, max)| max),
            cpu_sample: None,
        }
    }
}

/// Run all blocking I/O for a single server status check.
/// Designed to be called inside `tokio::task::spawn_blocking`.
pub fn compute_status_update(input: StatusRefreshInput) -> StatusUpdate {
    let status = detect_server_status(&input.server_type, input.port, &input.path);

    if !matches!(status, ServerStatus::Running) {
        return StatusUpdate {
            path: input.path,
            status,
            pid: None,
            ram_mb: None,
            cpu_percent: None,
            players_online: None,
            players_max: None,
            new_cpu_sample: None,
        };
    }

    let pid = if let Some(ref c) = input.container_name {
        find_pid_for_container(c)
    } else {
        find_process_pid(&input.server_type)
    };

    let (ram_mb, cpu_percent, new_cpu_sample) =
        if let Some(container) = input.container_name.as_deref() {
            get_docker_stats(container)
                .map(|(ram, cpu)| (Some(ram), Some(cpu), None))
                .unwrap_or((None, None, None))
        } else {
            let ram = pid.and_then(get_process_ram_mb);
            let new_sample = pid.and_then(read_cpu_snapshot);
            let cpu = new_sample.as_ref().and_then(|curr| {
                input
                    .prev_cpu_sample
                    .as_ref()
                    .and_then(|prev| calc_cpu_percent(prev, curr))
            });
            (ram, cpu, new_sample)
        };

    let players = match input.server_type {
        ServerType::Java => input.port.and_then(query_java_player_count),
        ServerType::Bedrock => input.port.and_then(query_bedrock_player_count),
        ServerType::Unknown => None,
    };

    StatusUpdate {
        path: input.path,
        status,
        pid,
        ram_mb,
        cpu_percent,
        players_online: players.map(|(online, _)| online),
        players_max: players.map(|(_, max)| max),
        new_cpu_sample,
    }
}

// NEW: Helper to determine status based on type and port
fn detect_server_status(server_type: &ServerType, port: Option<u16>, path: &Path) -> ServerStatus {
    match server_type {
        ServerType::Bedrock => {
            let udp_port = port.unwrap_or(19132);
            if is_udp_port_in_use(udp_port) {
                ServerStatus::Running
            } else {
                detect_server_process(path)
            }
        }

        ServerType::Java => {
            if let Some(p) = port {
                if is_port_in_use(p) {
                    ServerStatus::Running
                } else {
                    detect_server_process(path)
                }
            } else {
                detect_server_process(path)
            }
        }
        ServerType::Unknown => detect_server_process(path),
    }
}

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

fn detect_server_config(server_path: &Path) -> (ServerType, Option<u16>) {
    let bedrock_props = server_path.join("server.properties");
    let bedrock_exe = server_path.join("bedrock_server");
    let bedrock_exe_windows = server_path.join("bedrock_server.exe");

    if bedrock_props.exists() || bedrock_exe.exists() || bedrock_exe_windows.exists() {
        if let Some(port) = read_bedrock_port(&bedrock_props) {
            return (ServerType::Bedrock, Some(port));
        }
        return (ServerType::Bedrock, Some(19132));
    }

    let java_props = server_path.join("server.properties");
    let java_jar = server_path.join("server.jar");
    let paper_jar = server_path.join("paper.jar");
    let spigot_jar = server_path.join("spigot.jar");

    if java_props.exists() || java_jar.exists() || paper_jar.exists() || spigot_jar.exists() {
        if let Some(port) = read_java_port(&java_props) {
            return (ServerType::Java, Some(port));
        }
        return (ServerType::Java, Some(25565));
    }

    (ServerType::Unknown, None)
}

fn read_level_name(server_path: &Path) -> Option<String> {
    let content = fs::read_to_string(server_path.join("server.properties")).ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("level-name=") {
            let name = rest.trim().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

fn detect_worlds_root(server_path: &Path) -> PathBuf {
    let direct = server_path.join("worlds");
    if direct.exists() {
        return direct;
    }
    let data_worlds = server_path.join("data").join("worlds");
    if data_worlds.exists() {
        return data_worlds;
    }
    direct
}

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

fn is_port_in_use(port: u16) -> bool {
    TcpStream::connect_timeout(
        &format!("127.0.0.1:{}", port).parse().unwrap(),
        Duration::from_millis(100),
    )
    .is_ok()
}

fn is_udp_port_in_use(port: u16) -> bool {
    UdpSocket::bind(("0.0.0.0", port)).is_err()
}

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
            .args([
                "inspect",
                "--format",
                "{{range .Mounts}}{{.Source}} {{end}}",
                id,
            ])
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

pub fn read_docker_logs(container: &str, tail: usize) -> Vec<String> {
    let result = Command::new("docker")
        .args(["logs", "--tail", &tail.to_string(), container])
        .output();
    match result {
        Ok(o) => {
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

// ─── PID / RAM helpers ────────────────────────────────────────────────────────

fn find_pid_for_container(container: &str) -> Option<u32> {
    let out = Command::new("docker")
        .args(["inspect", "--format", "{{.State.Pid}}", container])
        .output()
        .ok()?;
    if out.status.success() {
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    } else {
        None
    }
}

fn find_process_pid(server_type: &ServerType) -> Option<u32> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let pattern = match server_type {
            ServerType::Bedrock => "bedrock_server",
            ServerType::Java => "java",
            ServerType::Unknown => return None,
        };
        let out = Command::new("pgrep").args(["-f", pattern]).output().ok()?;
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            return stdout.lines().next()?.trim().parse().ok();
        }
    }
    None
}

fn get_process_ram_mb(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                // "  12345 kB"
                let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kb / 1024);
            }
        }
    }
    None
}

fn get_docker_stats(container: &str) -> Option<(u64, f32)> {
    let out = Command::new("docker")
        .args([
            "stats",
            "--no-stream",
            "--format",
            "{{.CPUPerc}}|{{.MemUsage}}",
            container,
        ])
        .output()
        .ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout);
        let mut parts = s.trim().split('|');
        let cpu = parts.next().and_then(parse_docker_cpu_percent)?;
        let ram = parts.next().and_then(parse_docker_mem)?;
        Some((ram, cpu))
    } else {
        None
    }
}

fn parse_docker_cpu_percent(s: &str) -> Option<f32> {
    s.trim().strip_suffix('%')?.trim().parse::<f32>().ok()
}

fn parse_docker_mem(s: &str) -> Option<u64> {
    // Format: "123.4MiB / 1.938GiB"
    let usage = s.split('/').next()?.trim();
    if let Some(v) = usage.strip_suffix("GiB") {
        return v.trim().parse::<f64>().ok().map(|x| (x * 1024.0) as u64);
    }
    if let Some(v) = usage.strip_suffix("MiB") {
        return v.trim().parse::<f64>().ok().map(|x| x as u64);
    }
    if let Some(v) = usage.strip_suffix("KiB") {
        return v.trim().parse::<f64>().ok().map(|x| (x / 1024.0) as u64);
    }
    if let Some(v) = usage.strip_suffix("GB") {
        return v.trim().parse::<f64>().ok().map(|x| (x * 1000.0) as u64);
    }
    if let Some(v) = usage.strip_suffix("MB") {
        return v.trim().parse::<f64>().ok().map(|x| x as u64);
    }
    None
}

#[cfg(target_os = "linux")]
fn read_total_cpu_jiffies() -> Option<u64> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    let mut parts = stat.lines().next()?.split_whitespace();
    if parts.next()? != "cpu" {
        return None;
    }
    parts
        .filter_map(|p| p.parse::<u64>().ok())
        .try_fold(0_u64, |acc, v| acc.checked_add(v))
}

#[cfg(target_os = "linux")]
fn read_process_jiffies(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, after_cmd) = stat.rsplit_once(") ")?;
    let fields: Vec<&str> = after_cmd.split_whitespace().collect();
    let utime = fields.get(11)?.parse::<u64>().ok()?;
    let stime = fields.get(12)?.parse::<u64>().ok()?;
    utime.checked_add(stime)
}

#[cfg(target_os = "linux")]
fn read_cpu_snapshot(pid: u32) -> Option<CpuSnapshot> {
    Some(CpuSnapshot {
        process_jiffies: read_process_jiffies(pid)?,
        total_jiffies: read_total_cpu_jiffies()?,
    })
}

#[cfg(target_os = "linux")]
fn calc_cpu_percent(prev: &CpuSnapshot, current: &CpuSnapshot) -> Option<f32> {
    let proc_delta = current.process_jiffies.checked_sub(prev.process_jiffies)?;
    let total_delta = current.total_jiffies.checked_sub(prev.total_jiffies)?;
    if total_delta == 0 {
        return None;
    }
    let cores = std::thread::available_parallelism()
        .map(|n| n.get() as f32)
        .unwrap_or(1.0);
    Some((proc_delta as f32 / total_delta as f32) * 100.0 * cores)
}

#[cfg(not(target_os = "linux"))]
fn read_cpu_snapshot(_pid: u32) -> Option<CpuSnapshot> {
    None
}

#[cfg(not(target_os = "linux"))]
fn calc_cpu_percent(_prev: &CpuSnapshot, _current: &CpuSnapshot) -> Option<f32> {
    None
}

fn query_java_player_count(port: u16) -> Option<(u32, u32)> {
    let addr = format!("127.0.0.1:{port}");
    let mut stream =
        TcpStream::connect_timeout(&addr.parse().ok()?, Duration::from_millis(120)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(120)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(120)));

    let mut handshake = Vec::with_capacity(32);
    write_varint(&mut handshake, 0x00);
    write_varint(&mut handshake, 760);
    write_varint(&mut handshake, 9);
    handshake.extend_from_slice(b"localhost");
    handshake.extend_from_slice(&port.to_be_bytes());
    write_varint(&mut handshake, 1);
    write_packet(&mut stream, &handshake).ok()?;

    write_packet(&mut stream, &[0x00]).ok()?;

    let payload = read_packet(&mut stream)?;
    let mut cursor = 0;
    let packet_id = read_varint_from_slice(&payload, &mut cursor)?;
    if packet_id != 0x00 {
        return None;
    }
    let json_len = read_varint_from_slice(&payload, &mut cursor)? as usize;
    let remaining = payload.get(cursor..)?;
    if remaining.len() < json_len {
        return None;
    }
    let json = std::str::from_utf8(&remaining[..json_len]).ok()?;
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let players = value.get("players")?;
    let online = players.get("online")?.as_u64()? as u32;
    let max = players.get("max")?.as_u64()? as u32;
    Some((online, max))
}

fn query_bedrock_player_count(port: u16) -> Option<(u32, u32)> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    let _ = socket.set_read_timeout(Some(Duration::from_millis(120)));
    let _ = socket.set_write_timeout(Some(Duration::from_millis(120)));

    let mut packet = Vec::with_capacity(32);
    packet.push(0x01);
    packet.extend_from_slice(&0_i64.to_be_bytes());
    packet.extend_from_slice(&[
        0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56,
        0x78,
    ]);
    packet.extend_from_slice(&0_i64.to_be_bytes());

    socket.send_to(&packet, ("127.0.0.1", port)).ok()?;

    let mut buf = [0_u8; 2048];
    let (n, _) = socket.recv_from(&mut buf).ok()?;
    if n < 35 || buf[0] != 0x1c {
        return None;
    }

    let str_len = u16::from_be_bytes([buf[33], buf[34]]) as usize;
    if n < 35 + str_len {
        return None;
    }
    let motd = std::str::from_utf8(&buf[35..35 + str_len]).ok()?;
    let parts: Vec<&str> = motd.split(';').collect();
    let online = parts.get(4)?.parse::<u32>().ok()?;
    let max = parts.get(5)?.parse::<u32>().ok()?;
    Some((online, max))
}

fn write_varint(buf: &mut Vec<u8>, mut value: i32) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn read_varint_from_slice(input: &[u8], cursor: &mut usize) -> Option<i32> {
    let mut num_read = 0;
    let mut result = 0_i32;
    loop {
        let byte = *input.get(*cursor)?;
        let value = i32::from(byte & 0x7f);
        result |= value << (7 * num_read);
        num_read += 1;
        *cursor += 1;
        if num_read > 5 {
            return None;
        }
        if byte & 0x80 == 0 {
            break;
        }
    }
    Some(result)
}

fn write_packet(stream: &mut TcpStream, payload: &[u8]) -> std::io::Result<()> {
    let mut frame = Vec::with_capacity(payload.len() + 5);
    write_varint(&mut frame, payload.len() as i32);
    frame.extend_from_slice(payload);
    stream.write_all(&frame)
}

fn read_packet(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let length = read_varint_from_stream(stream)? as usize;
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).ok()?;
    Some(payload)
}

fn read_varint_from_stream(stream: &mut TcpStream) -> Option<i32> {
    let mut num_read = 0;
    let mut result = 0_i32;
    loop {
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte).ok()?;
        let value = i32::from(byte[0] & 0x7f);
        result |= value << (7 * num_read);
        num_read += 1;
        if num_read > 5 {
            return None;
        }
        if byte[0] & 0x80 == 0 {
            break;
        }
    }
    Some(result)
}

// ─── Server control ───────────────────────────────────────────────────────────

/// Send a command to a running server (Docker only for now).
pub fn send_server_command(instance: &ServerInstance, cmd: &str) -> Result<String, String> {
    if let Some(container) = &instance.container_name {
        // itzg/minecraft-bedrock-server ships a `send-command` helper
        let out = Command::new("docker")
            .args(["exec", container.as_str(), "send-command", cmd])
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(format!("Sent: {cmd}"))
        } else {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if err.is_empty() {
                format!("docker exec failed (exit {})", out.status)
            } else {
                err
            })
        }
    } else {
        Err("Command sending requires a Docker container. Non-Docker servers are not yet supported.".into())
    }
}

/// Restart a server (Docker containers or native processes).
pub fn restart_server(instance: &ServerInstance) -> Result<String, String> {
    if let Some(container) = &instance.container_name {
        let out = Command::new("docker")
            .args(["restart", container.as_str()])
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(format!("Restarted container '{container}'"))
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    } else if let Some(pid) = instance.pid {
        // For native processes: send SIGTERM then let the process restart itself
        // (works if the server is managed by a wrapper/service)
        #[cfg(target_os = "linux")]
        {
            let out = Command::new("kill")
                .args(["-SIGTERM", &pid.to_string()])
                .output()
                .map_err(|e| e.to_string())?;
            if out.status.success() {
                return Ok(format!(
                    "Sent SIGTERM to PID {pid}. Start the server manually if no auto-restart is configured."
                ));
            }
        }
        Err("Could not signal the server process.".into())
    } else {
        Err("No Docker container or running process found to restart.".into())
    }
}

// ─── server.properties editor ─────────────────────────────────────────────────

/// Read `server.properties` into editable key-value pairs (comments/blanks skipped).
pub fn read_server_properties(server_path: &Path) -> Vec<(String, String)> {
    let path = server_path.join("server.properties");
    let Ok(content) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let mut parts = l.splitn(2, '=');
            let key = parts.next()?.trim().to_string();
            let value = parts.next().unwrap_or("").trim().to_string();
            Some((key, value))
        })
        .collect()
}

/// Write `server.properties` preserving comment lines from the original file.
pub fn write_server_properties(
    server_path: &Path,
    props: &[(String, String)],
) -> color_eyre::Result<()> {
    let path = server_path.join("server.properties");
    let original = fs::read_to_string(&path).unwrap_or_default();

    // Build a lookup of new values
    let new_vals: std::collections::HashMap<&str, &str> = props
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let mut out = String::new();
    let mut seen = std::collections::HashSet::new();

    for line in original.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            out.push_str(line);
            out.push('\n');
        } else if let Some(eq) = line.find('=') {
            let key = line[..eq].trim();
            seen.insert(key.to_string());
            if let Some(val) = new_vals.get(key) {
                out.push_str(&format!("{key}={val}\n"));
            } else {
                out.push_str(line);
                out.push('\n');
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }

    // Append any keys not present in the original
    for (k, v) in props {
        if !seen.contains(k.as_str()) {
            out.push_str(&format!("{k}={v}\n"));
        }
    }

    // Back up then write
    let _ = fs::copy(&path, path.with_extension("properties.bak"));
    fs::write(&path, out)?;
    Ok(())
}

/// Detect if server process is running

fn detect_server_process(server_path: &Path) -> ServerStatus {
    let server_name = server_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();

    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = Command::new("pgrep")
            .args(["-f", "bedrock_server"])
            .output()
        {
            if output.status.success() {
                return ServerStatus::Running;
            }
        }
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
        if let Ok(output) = Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq bedrock_server.exe", "/FO", "CSV"])
            .output()
        {
            let output_str = String::from_utf8_lossy(&output.stdout);
            if output_str.contains("bedrock_server.exe") {
                return ServerStatus::Running;
            }
        }
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
        if let Ok(output) = Command::new("pgrep")
            .args(["-f", "bedrock_server"])
            .output()
        {
            if output.status.success() {
                return ServerStatus::Running;
            }
        }
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
