use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::{
    connection::ConnectionConfig,
    event::{AppEvent, Event, EventHandler},
    server::{discover_servers, read_docker_logs, ServerInstance},
    ui,
};

#[derive(Debug, PartialEq)]
pub enum AppMode {
    Normal,
    Installing { input: String },
    AddConnection { input: String, path_input: String, step: ConnectionStep },
    ManageConnections,
    RemoveConnection { selected: usize },
    ViewLogs { scroll: usize },
    ManagePacks { selected: usize },
}

#[derive(Debug, PartialEq)]
pub enum ConnectionStep {
    Name,
    Path,
}


#[derive(Debug)]
pub struct App {
    pub running: bool,
    pub servers: Vec<ServerInstance>,
    pub selected: usize,
    pub mode: AppMode,
    pub message: Option<String>,
    pub servers_path: PathBuf,
    pub events: EventHandler,
    pub connections: ConnectionConfig,
    pub selected_connection: usize,
    pub log_lines: Vec<String>,
}


impl App {
    pub fn new() -> Self {
        let connections = ConnectionConfig::load().unwrap_or_else(|_| ConnectionConfig {
            connections: Vec::new(),
        });

        let servers_path = PathBuf::from("servers");
        if !servers_path.exists() {
            let _ = std::fs::create_dir_all(&servers_path);
        }

        let servers = discover_servers_with_connections(&connections, &servers_path);

        let events = EventHandler::new();
        let sender = events.sender();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));
            loop {
                interval.tick().await;
                if sender.send(Event::App(AppEvent::UpdateStatuses)).is_err() {
                    break;
                }
            }

        });


        Self {
            running: true,
            servers,
            selected: 0,
            mode: AppMode::Normal,
            message: None,
            servers_path,
            events,
            connections,
            selected_connection: 0,
            log_lines: Vec::new(),
        }
    }

    pub async fn run(mut self, mut terminal: DefaultTerminal) -> color_eyre::Result<()> {
        while self.running {
            terminal.draw(|frame| ui::render(&self, frame))?;
            match self.events.next().await? {
                Event::Tick => {}

                Event::Crossterm(event) => {
                    if let crossterm::event::Event::Key(key) = event {
                        if key.kind == crossterm::event::KeyEventKind::Press {
                            self.handle_key(key)?;

                        }
                    }
                }
                Event::App(event) => self.handle_app_event(event),
            }
        }
        Ok(())
    }


    fn handle_key(&mut self, key: KeyEvent) -> color_eyre::Result<()> {
        match &self.mode {
            AppMode::Normal => self.handle_normal_key(key),
            AppMode::Installing { .. } => self.handle_install_key(key),
            AppMode::AddConnection { .. } => self.handle_add_connection_key(key),
            AppMode::ManageConnections => self.handle_manage_connections_key(key),
            AppMode::RemoveConnection { .. } => self.handle_remove_connection_key(key),
            AppMode::ViewLogs { .. } => self.handle_view_logs_key(key),
            AppMode::ManagePacks { .. } => self.handle_manage_packs_key(key),
        }
        Ok(())
    }


    fn handle_normal_key(&mut self, key: KeyEvent) {
        self.message = None;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.events.send(AppEvent::Quit),
            KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                self.events.send(AppEvent::Quit)
            }

            KeyCode::Up => self.events.send(AppEvent::SelectPrev),
            KeyCode::Down => self.events.send(AppEvent::SelectNext),
            KeyCode::Char('i') => {
                if !self.servers.is_empty() {
                    self.mode = AppMode::Installing {
                        input: String::new(),
                    };
                } else {
                    self.message =
                        Some("No servers found. Add a connection with 'a' first.".into());
                }
            }
            KeyCode::Char('a') => {
                self.mode = AppMode::AddConnection { 
                    input: String::new(), 
                    path_input: String::new(),
                    step: ConnectionStep::Name,
                };
            }
            KeyCode::Char('m') => {
                self.mode = AppMode::ManageConnections;
                self.selected_connection = 0;

            }
            KeyCode::Char('p') => {

                if !self.servers.is_empty() {
                    self.mode = AppMode::ManagePacks { selected: 0 };
                }
            }
            KeyCode::Char('r') => {
                self.refresh_servers();

                self.message = Some("Server list refreshed.".into());
            }
            KeyCode::Char('l') => {

                if self.servers.is_empty() {
                    return;
                }
                let container = self.servers[self.selected].container_name.clone();
                match container {
                    Some(c) => {
                        self.mode = AppMode::ViewLogs { scroll: usize::MAX };
                        self.message = Some("Loading logs…".into());
                        self.run_load_logs(c);
                    }
                    None => {
                        self.message = Some("No Docker container linked to this server.".into());
                    }

                }
            }
            _ => {}
        }
    }

    fn handle_install_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = AppMode::Normal;
                self.message = None;
            }
            KeyCode::Enter => {
                let input = match &self.mode {
                    AppMode::Installing { input } => input.clone(),
                    _ => return,
                };
                let path = PathBuf::from(input.trim());
                self.mode = AppMode::Normal;
                if path.exists() {
                    self.events.send(AppEvent::InstallPlugin(path));

                    self.message = Some("Installing…".into());
                } else {
                    self.message = Some(format!("Path not found: {}", input.trim()));
                }
            }
            KeyCode::Backspace => {
                if let AppMode::Installing { input } = &mut self.mode {
                    input.pop();
                }
            }
            KeyCode::Char(c) => {
                if let AppMode::Installing { input } = &mut self.mode {
                    input.push(c);
                }
            }
            _ => {}
        }
    }

    fn handle_add_connection_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = AppMode::Normal;
                self.message = None;
            }
            KeyCode::Enter => {
                let (name, path_str, step) = match &self.mode {
                    AppMode::AddConnection { input, path_input, step } => {
                        (input.clone(), path_input.clone(), step)
                    }
                    _ => return,
                };
                match step {
                    ConnectionStep::Name => {
                        if name.trim().is_empty() {
                            self.message = Some("Connection name cannot be empty".into());
                            return;
                        }
                        self.mode = AppMode::AddConnection {
                            input: name,
                            path_input: String::new(),
                            step: ConnectionStep::Path,
                        };
                    }
                    ConnectionStep::Path => {
                        let path = PathBuf::from(path_str.trim());
                        match self.connections.add_connection(name.clone(), path) {
                            Ok(_) => {
                                self.message = Some(format!("Added connection: {}", name));
                                self.mode = AppMode::Normal;

                                self.refresh_servers();
                            }
                            Err(e) => {
                                self.message = Some(format!("Error: {}", e));
                                self.mode = AppMode::Normal;
                            }
                        }
                    }
                }
            }
            KeyCode::Backspace => {
                match &mut self.mode {
                    AppMode::AddConnection { input, path_input, step } => {
                        match step {
                            ConnectionStep::Name => { input.pop(); }
                            ConnectionStep::Path => { path_input.pop(); }
                        }

                    }
                    _ => {}
                }
            }
            KeyCode::Char(c) => {
                match &mut self.mode {
                    AppMode::AddConnection { input, path_input, step } => {
                        match step {
                            ConnectionStep::Name => input.push(c),
                            ConnectionStep::Path => path_input.push(c),
                        }
                    }

                    _ => {}
                }
            }

            _ => {}
        }
    }


    fn handle_manage_connections_key(&mut self, key: KeyEvent) {

        match key.code {
            KeyCode::Esc => {
                self.mode = AppMode::Normal;
            }

            KeyCode::Up => {
                if self.selected_connection > 0 {
                    self.selected_connection -= 1;
                }
            }
            KeyCode::Down => {
                if self.selected_connection + 1 < self.connections.connections.len() {

                    self.selected_connection += 1;
                }
            }
            KeyCode::Char('d') => {
                if !self.connections.connections.is_empty() {
                    self.mode = AppMode::RemoveConnection {
                        selected: self.selected_connection,
                    };
                }
            }
            KeyCode::Enter => {
                if !self.connections.connections.is_empty() {
                    let conn = &self.connections.connections[self.selected_connection];

                    self.message = Some(format!(
                        "Connection: {} → {} {}", 
                        conn.name, 
                        conn.path.display(),
                        if conn.is_symlink { "(symlink)" } else { "" }
                    ));
                }
            }
            _ => {}
        }
    }

    fn handle_remove_connection_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = AppMode::ManageConnections;
            }
            KeyCode::Char('y') | KeyCode::Enter => {
                if let AppMode::RemoveConnection { selected } = self.mode {
                    let name = self.connections.connections[selected].name.clone();
                    if let Err(e) = self.connections.remove_connection(selected) {
                        self.message = Some(format!("Error removing connection: {}", e));
                    } else {

                        self.message = Some(format!("Removed connection: {}", name));
                        self.refresh_servers();
                    }
                }
                self.mode = AppMode::ManageConnections;
            }

            KeyCode::Char('n') | KeyCode::Char('q') => {
                self.mode = AppMode::ManageConnections;
            }
            _ => {}
        }
    }


    fn handle_view_logs_key(&mut self, key: KeyEvent) {
        match key.code {

            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = AppMode::Normal;
            }
            KeyCode::Down => {
                if let AppMode::ViewLogs { scroll } = &mut self.mode {
                    *scroll = scroll.saturating_add(1);
                }
            }

            KeyCode::Up => {
                if let AppMode::ViewLogs { scroll } = &mut self.mode {
                    *scroll = scroll.saturating_sub(1);
                }
            }
            KeyCode::PageDown => {
                if let AppMode::ViewLogs { scroll } = &mut self.mode {
                    *scroll = scroll.saturating_add(20);
                }
            }
            KeyCode::PageUp => {
                if let AppMode::ViewLogs { scroll } = &mut self.mode {

                    *scroll = scroll.saturating_sub(20);
                }
            }
            _ => {}
        }
    }


    // --- Manage Packs Methods ---

    /// Returns the total number of visual lines in the Manage Packs view for the selected server.
    fn manage_packs_total_visual(&self) -> usize {
        if self.servers.is_empty() {
            return 0;
        }
        let server = &self.servers[self.selected];
        let rp_len = server.installed_resource_packs.len();
        let bp_len = server.installed_behavior_packs.len();


        // Structure:
        // 1. Resource Packs header
        // 2. Resource packs content (rp_len if >0 else 1 for "(none)")
        // 3. Empty line separator
        // 4. Behavior Packs header
        // 5. Behavior packs content (bp_len if >0 else 1 for "(none)")
        let mut total = 5;
        total += if rp_len > 0 { rp_len - 1 } else { 0 };
        total += if bp_len > 0 { bp_len - 1 } else { 0 };
        total
    }

    fn handle_manage_packs_key(&mut self, key: KeyEvent) {
        let total = self.manage_packs_total_visual();

        match key.code {
            KeyCode::Esc => {
                self.mode = AppMode::Normal;
            }
            KeyCode::Up => {
                if let AppMode::ManagePacks { selected } = &mut self.mode {
                    *selected = selected.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if let AppMode::ManagePacks { selected } = &mut self.mode {
                    if *selected + 1 < total {
                        *selected += 1;
                    }
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let idx = if let AppMode::ManagePacks { selected } = &self.mode {

                    *selected
                } else {
                    return;
                };
                self.toggle_pack(idx);
            }
            _ => {}
        }
    }

    fn toggle_pack(&mut self, visual_idx: usize) {
        if self.servers.is_empty() {
            return;
        }
        let server = &self.servers[self.selected];
        let rp_len = server.installed_resource_packs.len();
        let bp_len = server.installed_behavior_packs.len();


        // Map visual index to pack by simulating the list structure
        let mut current = 0;

        // Resource header
        current += 1;

        // Resource packs
        if rp_len == 0 {
            if visual_idx == current {
                return; // "(none installed)" line – do nothing
            }
            current += 1;

        } else {
            for i in 0..rp_len {
                if visual_idx == current {
                    self.toggle_pack_by_index(true, i);

                    return;
                }
                current += 1;
            }
        }

        // Empty line separator

        if visual_idx == current {
            return;
        }
        current += 1;

        // Behavior header
        if visual_idx == current {
            return;
        }
        current += 1;

        // Behavior packs
        if bp_len == 0 {
            if visual_idx == current {
                return; // "(none installed)" line

            }
            // current += 1; // not needed
        } else {
            for i in 0..bp_len {
                if visual_idx == current {

                    self.toggle_pack_by_index(false, i);

                    return;
                }
                current += 1;
            }
        }
    }

    fn toggle_pack_by_index(&mut self, is_resource: bool, idx: usize) {
        let server = &self.servers[self.selected];

        let (json_path, uuid, version, currently_enabled) = if is_resource {
            let pack = &server.installed_resource_packs[idx];

            (
                server.path.join("resource_packs.json"),
                pack.uuid.clone(),

                pack.version.clone(),
                pack.enabled,
            )
        } else {

            let pack = &server.installed_behavior_packs[idx];
            (
                server.path.join("behavior_packs.json"),
                pack.uuid.clone(),
                pack.version.clone(),
                pack.enabled,
            )
        };

        match crate::plugin::installer::set_pack_enabled(
            &json_path,

            &uuid,
            &version,
            !currently_enabled,
        ) {
            Ok(()) => {
                self.message = Some(if currently_enabled {
                    format!("Disabled '{uuid}'")

                } else {
                    format!("Enabled '{uuid}'")
                });
            }

            Err(e) => {
                self.message = Some(format!("Toggle error: {e}"));

            }
        }

        self.refresh_servers();
    }

    // --- End Manage Packs Methods ---


    fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Quit => self.running = false,
            AppEvent::SelectNext => {
                if !self.servers.is_empty() {
                    self.selected = (self.selected + 1).min(self.servers.len() - 1);
                }
            }
            AppEvent::SelectPrev => {
                self.selected = self.selected.saturating_sub(1);

            }
            AppEvent::InstallPlugin(path) => self.run_install(path),
            AppEvent::InstallDone(result) => match result {
                Ok(msg) => {

                    self.message = Some(msg);
                    self.refresh_servers();
                }
                Err(err) => {
                    self.message = Some(format!("Install error: {err}"));
                }
            },
            AppEvent::LogsLoaded(lines) => {
                self.log_lines = lines;
                self.message = None;
                if let AppMode::ViewLogs { scroll } = &mut self.mode {
                    *scroll = self.log_lines.len().saturating_sub(1);
                }

            }
            AppEvent::UpdateStatuses => self.update_statuses(),
        }
    }

    fn refresh_servers(&mut self) {
        self.servers = discover_servers_with_connections(&self.connections, &self.servers_path);
        self.selected = self.selected.min(self.servers.len().saturating_sub(1));
    }

    fn update_statuses(&mut self) {
        for server in &mut self.servers {
            server.refresh_status();
        }
    }

    fn run_load_logs(&self, container: String) {
        let sender = self.events.sender();

        tokio::spawn(async move {

            let lines = tokio::task::spawn_blocking(move || read_docker_logs(&container, 300))
                .await

                .unwrap_or_default();
            let _ = sender.send(Event::App(AppEvent::LogsLoaded(lines)));
        });
    }

    fn run_install(&self, path: PathBuf) {
        let server_path = self.servers[self.selected].path.clone();
        let sender = self.events.sender();
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                crate::plugin::installer::install(&path, &server_path)

                    .map(|results| {
                        let names = results
                            .iter()
                            .map(|r| format!("'{}' ({})", r.pack_name, r.pack_type))
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("Installed: {names}")
                    })
                    .map_err(|e| e.to_string())
            })
            .await;
            let msg = match result {
                Ok(r) => r,
                Err(e) => Err(e.to_string()),

            };
            let _ = sender.send(Event::App(AppEvent::InstallDone(msg)));
        });
    }
}

fn discover_servers_with_connections(connections: &ConnectionConfig, legacy_path: &Path) -> Vec<ServerInstance> {
    let mut servers = Vec::new();

    for conn in &connections.connections {
        if conn.path.exists() {
            let server = ServerInstance::from_path(&conn.path, Some(&conn.name));
            servers.push(server);
        }
    }

    if legacy_path.exists() {
        let legacy_servers = discover_servers(legacy_path);
        for server in legacy_servers {
            if !servers.iter().any(|s| s.path == server.path) {
                servers.push(server);
            }
        }
    }
    servers.sort_by(|a, b| a.name.cmp(&b.name));

    servers
}
