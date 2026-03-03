use std::path::{Path, PathBuf};



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
    /// Status/error message shown in the status bar.
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
        
        // Create servers directory if it doesn't exist (for backward compatibility)
        let servers_path = PathBuf::from("servers");
        if !servers_path.exists() {
            let _ = std::fs::create_dir_all(&servers_path);
        }

        let servers = discover_servers_with_connections(&connections, &servers_path);
        
        Self {
            running: true,
            servers,
            selected: 0,
            mode: AppMode::Normal,
            message: None,
            servers_path,
            events: EventHandler::new(),
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
        self.message = None; // clear message on any keypress
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
                    // View/edit connection details
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

    fn handle_manage_packs_key(&mut self, key: KeyEvent) {
        let total = if self.servers.is_empty() {
            0
        } else {
            let s = &self.servers[self.selected];
            s.installed_resource_packs.len() + s.installed_behavior_packs.len()
        };

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

    fn toggle_pack(&mut self, selected: usize) {
        if self.servers.is_empty() {
            return;
        }
        let server = &self.servers[self.selected];
        let rp_len = server.installed_resource_packs.len();

        let (json_path, uuid, version, currently_enabled) = if selected < rp_len {
            let pack = &server.installed_resource_packs[selected];
            (
                server.path.join("resource_packs.json"),
                pack.uuid.clone(),
                pack.version.clone(),
                pack.enabled,
            )
        } else {
            let pack = &server.installed_behavior_packs[selected - rp_len];
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
                // Scroll to bottom so the most recent entries are visible
                if let AppMode::ViewLogs { scroll } = &mut self.mode {
                    *scroll = self.log_lines.len().saturating_sub(1);
                }
            }
        }
    }

    fn refresh_servers(&mut self) {
        self.servers = discover_servers_with_connections(&self.connections, &self.servers_path);
        self.selected = self.selected.min(self.servers.len().saturating_sub(1));
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
    
    // Add servers from connections
    for conn in &connections.connections {
        if conn.path.exists() {
            let server = ServerInstance::from_path(&conn.path, Some(&conn.name));
            servers.push(server);
        }
    }
    
    // Also include legacy servers from ./servers/ for backward compatibility
    if legacy_path.exists() {
        let legacy_servers = discover_servers(legacy_path);
        for server in legacy_servers {
            // Avoid duplicates
            if !servers.iter().any(|s| s.path == server.path) {
                servers.push(server);
            }
        }
    }
    
    servers.sort_by(|a, b| a.name.cmp(&b.name));
    servers
}
