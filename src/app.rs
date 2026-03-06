use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::{
    connection::ConnectionConfig,
    event::{AppEvent, Event, EventHandler},
    server::{
        ServerInstance, ServerStatus, StatusRefreshInput, compute_status_update, discover_servers,
        read_docker_logs, read_server_properties, restart_server, send_server_command,
        write_server_properties,
    },
    ui,
};

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum InstallStep {
    Path,
    WorldAction,
    Name,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum InstallKind {
    Plugin,
    WorldCreate,
    WorldModify,
}

#[derive(Debug, PartialEq)]
pub enum AppMode {
    Normal,
    Installing {
        step: InstallStep,
        install_kind: InstallKind,
        path_input: String,
        name_input: String,
    },
    AddConnection {
        input: String,
        path_input: String,
        container_input: String,
        step: ConnectionStep,
    },
    ManageConnections,
    RemoveConnection {
        selected: usize,
    },
    ViewLogs {
        scroll: usize,
    },
    ManagePacks {
        selected: usize,
        moving: bool,
    },
    /// Modal to type and send a command to the selected server.
    SendCommand {
        input: String,
    },
    /// In-panel editor for server.properties.
    EditConfig {
        props: Vec<(String, String)>,
        selected: usize,
        editing: bool,
        edit_input: String,
    },
}

#[derive(Debug, PartialEq)]
pub enum ConnectionStep {
    Name,
    Path,
    Container,
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
    pub tick_count: u64,
    /// Guards against multiple concurrent status-refresh tasks.
    status_refresh_pending: bool,
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
            tick_count: 0,
            status_refresh_pending: false,
        }
    }

    pub async fn run(mut self, mut terminal: DefaultTerminal) -> color_eyre::Result<()> {
        while self.running {
            terminal.draw(|frame| ui::render(&self, frame))?;
            match self.events.next().await? {
                Event::Tick => {
                    self.tick_count = self.tick_count.wrapping_add(1);
                    // Full rediscovery is heavier; keep it infrequent and rely on UpdateStatuses
                    // for lightweight live metrics.
                    if self.tick_count % 900 == 0 {
                        self.run_auto_refresh();
                    }
                }
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
            AppMode::SendCommand { .. } => self.handle_send_command_key(key),
            AppMode::EditConfig { .. } => self.handle_edit_config_key(key),
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

            KeyCode::Up | KeyCode::Char('k') => self.events.send(AppEvent::SelectPrev),
            KeyCode::Down | KeyCode::Char('j') => self.events.send(AppEvent::SelectNext),
            KeyCode::Char('i') => {
                if !self.servers.is_empty() {
                    self.mode = AppMode::Installing {
                        step: InstallStep::Path,
                        install_kind: InstallKind::Plugin,
                        path_input: String::new(),
                        name_input: String::new(),
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
                    container_input: String::new(),
                    step: ConnectionStep::Name,
                };
            }
            KeyCode::Char('m') => {
                self.mode = AppMode::ManageConnections;
                self.selected_connection = 0;
            }
            KeyCode::Char('p') => {
                if !self.servers.is_empty() {
                    self.mode = AppMode::ManagePacks {
                        selected: 0,
                        moving: false,
                    };
                }
            }
            KeyCode::Char('r') => {
                self.run_auto_refresh();
                self.message = Some("Refreshing…".into());
            }
            // Send command to running server
            KeyCode::Char('c') => {
                if self.servers.is_empty() {
                    return;
                }
                if matches!(self.servers[self.selected].status, ServerStatus::Running) {
                    self.mode = AppMode::SendCommand {
                        input: String::new(),
                    };
                } else {
                    self.message = Some("Server is not running.".into());
                }
            }
            // Restart server (capital R = Shift+r)
            KeyCode::Char('R') => {
                if self.servers.is_empty() {
                    return;
                }
                let instance = self.servers[self.selected].clone();
                let sender = self.events.sender();
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(move || restart_server(&instance))
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()));
                    let _ = sender.send(Event::App(AppEvent::ServerRestarted(result)));
                });
                self.message = Some("Restarting…".into());
            }
            // Edit server.properties
            KeyCode::Char('e') => {
                if self.servers.is_empty() {
                    return;
                }
                let props = read_server_properties(&self.servers[self.selected].path);
                if props.is_empty() {
                    self.message = Some("No server.properties found.".into());
                } else {
                    self.mode = AppMode::EditConfig {
                        props,
                        selected: 0,
                        editing: false,
                        edit_input: String::new(),
                    };
                }
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
                let (step, install_kind, path_input, name_input) = match &self.mode {
                    AppMode::Installing {
                        step,
                        install_kind,
                        path_input,
                        name_input,
                    } => (*step, *install_kind, path_input.clone(), name_input.clone()),
                    _ => return,
                };
                match step {
                    InstallStep::Path => {
                        if path_input.trim().is_empty() {
                            self.message = Some("Path cannot be empty".into());
                            return;
                        }
                        let path = PathBuf::from(path_input.trim());
                        if !path.exists() {
                            self.message = Some(format!("Path not found: {}", path_input.trim()));
                            return;
                        }

                        if is_mcworld_path(&path) {
                            self.mode = AppMode::Installing {
                                step: InstallStep::WorldAction,
                                install_kind: InstallKind::WorldCreate,
                                path_input,
                                name_input: String::new(),
                            };
                        } else {
                            self.mode = AppMode::Installing {
                                step: InstallStep::Name,
                                install_kind: InstallKind::Plugin,
                                path_input,
                                name_input: String::new(),
                            };
                        }
                    }
                    InstallStep::WorldAction => {
                        self.message =
                            Some("Choose world action: press 'c' (create) or 'm' (modify)".into());
                    }
                    InstallStep::Name => {
                        let path = PathBuf::from(path_input.trim());
                        if !path.exists() {
                            self.message = Some(format!("Path not found: {}", path_input.trim()));
                            self.mode = AppMode::Normal;
                            return;
                        }
                        match install_kind {
                            InstallKind::Plugin => {
                                let custom_name = if name_input.trim().is_empty() {
                                    None
                                } else {
                                    Some(name_input.trim().to_string())
                                };
                                self.events.send(AppEvent::InstallPlugin(path, custom_name));
                                self.message = Some("Installing…".into());
                            }
                            InstallKind::WorldCreate => {
                                let world_name = if name_input.trim().is_empty() {
                                    None
                                } else {
                                    Some(name_input.trim().to_string())
                                };
                                self.events.send(AppEvent::ImportWorld(
                                    path,
                                    crate::world::WorldImportMode::Create,
                                    world_name,
                                ));
                                self.message = Some("Importing world…".into());
                            }
                            InstallKind::WorldModify => {
                                if name_input.trim().is_empty() {
                                    self.message = Some(
                                        "Target world name is required for modify mode".into(),
                                    );
                                    return;
                                }
                                self.events.send(AppEvent::ImportWorld(
                                    path,
                                    crate::world::WorldImportMode::Modify,
                                    Some(name_input.trim().to_string()),
                                ));
                                self.message = Some("Updating world…".into());
                            }
                        }
                        self.mode = AppMode::Normal;
                    }
                }
            }
            KeyCode::Backspace => match &mut self.mode {
                AppMode::Installing {
                    step,
                    path_input,
                    name_input,
                    ..
                } => match step {
                    InstallStep::Path => {
                        path_input.pop();
                    }
                    InstallStep::WorldAction => {}
                    InstallStep::Name => {
                        name_input.pop();
                    }
                },
                _ => {}
            },
            KeyCode::Char(c) => {
                let current = match &self.mode {
                    AppMode::Installing {
                        step, path_input, ..
                    } => Some((*step, path_input.clone())),
                    _ => None,
                };
                match current {
                    Some((InstallStep::Path, _)) => {
                        if let AppMode::Installing { path_input, .. } = &mut self.mode {
                            path_input.push(c);
                        }
                    }
                    Some((InstallStep::WorldAction, path_input)) => {
                        if matches!(c, 'c' | 'C') {
                            self.mode = AppMode::Installing {
                                step: InstallStep::Name,
                                install_kind: InstallKind::WorldCreate,
                                path_input,
                                name_input: String::new(),
                            };
                        } else if matches!(c, 'm' | 'M') {
                            self.mode = AppMode::Installing {
                                step: InstallStep::Name,
                                install_kind: InstallKind::WorldModify,
                                path_input,
                                name_input: String::new(),
                            };
                        }
                    }
                    Some((InstallStep::Name, _)) => {
                        if let AppMode::Installing { name_input, .. } = &mut self.mode {
                            name_input.push(c);
                        }
                    }
                    None => {}
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
                let (name, path_str, container_input, step) = match &self.mode {
                    AppMode::AddConnection {
                        input,
                        path_input,
                        container_input,
                        step,
                    } => (
                        input.clone(),
                        path_input.clone(),
                        container_input.clone(),
                        step,
                    ),
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
                            container_input: String::new(),
                            step: ConnectionStep::Path,
                        };
                    }
                    ConnectionStep::Path => {
                        if path_str.trim().is_empty() {
                            self.message = Some("Server path cannot be empty".into());
                            return;
                        }
                        self.mode = AppMode::AddConnection {
                            input: name,
                            path_input: path_str,
                            container_input: String::new(),
                            step: ConnectionStep::Container,
                        };
                    }
                    ConnectionStep::Container => {
                        let path = PathBuf::from(path_str.trim());
                        let container_name = if container_input.trim().is_empty() {
                            None
                        } else {
                            Some(container_input.trim().to_string())
                        };
                        match self
                            .connections
                            .add_connection(name.clone(), path, container_name)
                        {
                            Ok(_) => {
                                self.message = Some(format!("Added connection: {}", name));
                                self.mode = AppMode::Normal;
                                self.run_auto_refresh();
                            }
                            Err(e) => {
                                self.message = Some(format!("Error: {}", e));
                                self.mode = AppMode::Normal;
                            }
                        }
                    }
                }
            }
            KeyCode::Backspace => match &mut self.mode {
                AppMode::AddConnection {
                    input,
                    path_input,
                    container_input,
                    step,
                } => match step {
                    ConnectionStep::Name => {
                        input.pop();
                    }
                    ConnectionStep::Path => {
                        path_input.pop();
                    }
                    ConnectionStep::Container => {
                        container_input.pop();
                    }
                },
                _ => {}
            },
            KeyCode::Char(c) => match &mut self.mode {
                AppMode::AddConnection {
                    input,
                    path_input,
                    container_input,
                    step,
                } => match step {
                    ConnectionStep::Name => input.push(c),
                    ConnectionStep::Path => path_input.push(c),
                    ConnectionStep::Container => container_input.push(c),
                },

                _ => {}
            },

            _ => {}
        }
    }

    fn handle_manage_connections_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = AppMode::Normal;
            }

            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected_connection > 0 {
                    self.selected_connection -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
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
                    let container = conn
                        .container_name
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("(auto-detect)");

                    self.message = Some(format!(
                        "Connection: {} → {} {} container: {}",
                        conn.name,
                        conn.path.display(),
                        if conn.is_symlink { "(symlink)" } else { "" },
                        container
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
                        self.run_auto_refresh();
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
            KeyCode::Down | KeyCode::Char('j') => {
                if let AppMode::ViewLogs { scroll } = &mut self.mode {
                    *scroll = scroll.saturating_add(1);
                }
            }

            KeyCode::Up | KeyCode::Char('k') => {
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
        let (selected, moving) = if let AppMode::ManagePacks { selected, moving } = &self.mode {
            (*selected, *moving)
        } else {
            return;
        };

        match key.code {
            KeyCode::Esc => {
                if moving {
                    self.mode = AppMode::ManagePacks {
                        selected,
                        moving: false,
                    };
                } else {
                    self.mode = AppMode::Normal;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if moving {
                    if self.reorder_pack_by_selected(
                        selected,
                        crate::plugin::installer::ReorderDirection::Up,
                    ) {
                        self.mode = AppMode::ManagePacks {
                            selected: selected.saturating_sub(1),
                            moving: true,
                        };
                    }
                } else if let AppMode::ManagePacks { selected, .. } = &mut self.mode {
                    *selected = selected.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if moving {
                    if self.reorder_pack_by_selected(
                        selected,
                        crate::plugin::installer::ReorderDirection::Down,
                    ) {
                        self.mode = AppMode::ManagePacks {
                            selected: (selected + 1).min(total.saturating_sub(1)),
                            moving: true,
                        };
                    }
                } else if let AppMode::ManagePacks { selected, .. } = &mut self.mode {
                    if *selected + 1 < total {
                        *selected += 1;
                    }
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if moving {
                    self.mode = AppMode::ManagePacks {
                        selected,
                        moving: false,
                    };
                } else {
                    self.toggle_pack(selected);
                }
            }
            KeyCode::Char('m') => {
                if moving {
                    self.mode = AppMode::ManagePacks {
                        selected,
                        moving: false,
                    };
                    return;
                }
                let Some((is_resource, idx)) = self.manage_pack_target(selected) else {
                    self.message = Some("Select a pack row first.".into());
                    return;
                };
                let server = &self.servers[self.selected];
                let pack = if is_resource {
                    &server.installed_resource_packs[idx]
                } else {
                    &server.installed_behavior_packs[idx]
                };
                if !pack.enabled {
                    self.message = Some("Enable the pack first to move it.".into());
                    return;
                }
                self.mode = AppMode::ManagePacks {
                    selected,
                    moving: true,
                };
                self.message = Some(format!("Move mode: '{}'", pack.name));
            }
            KeyCode::Char('K') => {
                self.reorder_pack_by_selected(
                    selected,
                    crate::plugin::installer::ReorderDirection::Up,
                );
            }
            KeyCode::Char('J') => {
                self.reorder_pack_by_selected(
                    selected,
                    crate::plugin::installer::ReorderDirection::Down,
                );
            }
            _ => {}
        }
    }

    fn handle_send_command_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = AppMode::Normal;
            }
            KeyCode::Enter => {
                let input = match &self.mode {
                    AppMode::SendCommand { input } => input.clone(),
                    _ => return,
                };
                self.mode = AppMode::Normal;
                if input.trim().is_empty() {
                    return;
                }
                let instance = self.servers[self.selected].clone();
                let sender = self.events.sender();
                tokio::spawn(async move {
                    let result =
                        tokio::task::spawn_blocking(move || send_server_command(&instance, &input))
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()));
                    let _ = sender.send(Event::App(AppEvent::CommandSent(result)));
                });
                self.message = Some("Sending command…".into());
            }
            KeyCode::Backspace => {
                if let AppMode::SendCommand { input } = &mut self.mode {
                    input.pop();
                }
            }
            KeyCode::Char(c) => {
                if let AppMode::SendCommand { input } = &mut self.mode {
                    input.push(c);
                }
            }
            _ => {}
        }
    }

    fn handle_edit_config_key(&mut self, key: KeyEvent) {
        let (props_len, is_editing) = match &self.mode {
            AppMode::EditConfig { props, editing, .. } => (props.len(), *editing),
            _ => return,
        };

        if is_editing {
            match key.code {
                KeyCode::Esc => {
                    if let AppMode::EditConfig {
                        editing,
                        edit_input,
                        ..
                    } = &mut self.mode
                    {
                        *editing = false;
                        edit_input.clear();
                    }
                }
                KeyCode::Enter => {
                    if let AppMode::EditConfig {
                        props,
                        selected,
                        editing,
                        edit_input,
                    } = &mut self.mode
                    {
                        let new_val = edit_input.clone();
                        props[*selected].1 = new_val;
                        *editing = false;
                        edit_input.clear();
                    }
                }
                KeyCode::Backspace => {
                    if let AppMode::EditConfig { edit_input, .. } = &mut self.mode {
                        edit_input.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if let AppMode::EditConfig { edit_input, .. } = &mut self.mode {
                        edit_input.push(c);
                    }
                }
                _ => {}
            }
        } else {
            match key.code {
                KeyCode::Esc => {
                    self.mode = AppMode::Normal;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if let AppMode::EditConfig { selected, .. } = &mut self.mode {
                        *selected = selected.saturating_sub(1);
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if let AppMode::EditConfig { selected, .. } = &mut self.mode {
                        if *selected + 1 < props_len {
                            *selected += 1;
                        }
                    }
                }
                KeyCode::Enter => {
                    if let AppMode::EditConfig {
                        props,
                        selected,
                        editing,
                        edit_input,
                    } = &mut self.mode
                    {
                        *edit_input = props[*selected].1.clone();
                        *editing = true;
                    }
                }
                KeyCode::Char('s') => {
                    let (props, server_path) = match &self.mode {
                        AppMode::EditConfig { props, .. } => {
                            (props.clone(), self.servers[self.selected].path.clone())
                        }
                        _ => return,
                    };
                    match write_server_properties(&server_path, &props) {
                        Ok(()) => {
                            self.message = Some("Config saved.".into());
                            self.mode = AppMode::Normal;
                        }
                        Err(e) => {
                            self.message = Some(format!("Save error: {e}"));
                            self.mode = AppMode::Normal;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn toggle_pack(&mut self, selected: usize) {
        if let Some((is_resource, idx)) = self.manage_pack_target(selected) {
            self.toggle_pack_by_index(is_resource, idx);
        }
    }

    fn reorder_pack_by_selected(
        &mut self,
        selected: usize,
        direction: crate::plugin::installer::ReorderDirection,
    ) -> bool {
        let Some((is_resource, idx)) = self.manage_pack_target(selected) else {
            return false;
        };

        let (uuid, name, enabled) = {
            let server = &self.servers[self.selected];
            let pack = if is_resource {
                &server.installed_resource_packs[idx]
            } else {
                &server.installed_behavior_packs[idx]
            };
            (pack.uuid.clone(), pack.name.clone(), pack.enabled)
        };

        if !enabled {
            self.message = Some("Pack is disabled. Enable it first to reorder.".into());
            return false;
        }

        let server_path = self.servers[self.selected].path.clone();
        match crate::plugin::installer::reorder_pack(&server_path, &uuid, is_resource, direction) {
            Ok(true) => {
                let dir = match direction {
                    crate::plugin::installer::ReorderDirection::Up => "up",
                    crate::plugin::installer::ReorderDirection::Down => "down",
                };
                self.message = Some(format!("Moved '{name}' {dir}"));
                self.apply_local_pack_reorder(is_resource, idx, direction);
                true
            }
            Ok(false) => {
                self.message = Some("Cannot move further in that direction.".into());
                false
            }
            Err(e) => {
                self.message = Some(format!("Reorder error: {e}"));
                false
            }
        }
    }

    fn apply_local_pack_reorder(
        &mut self,
        is_resource: bool,
        idx: usize,
        direction: crate::plugin::installer::ReorderDirection,
    ) {
        let packs = if is_resource {
            &mut self.servers[self.selected].installed_resource_packs
        } else {
            &mut self.servers[self.selected].installed_behavior_packs
        };

        let target = match direction {
            crate::plugin::installer::ReorderDirection::Up if idx > 0 => Some(idx - 1),
            crate::plugin::installer::ReorderDirection::Down if idx + 1 < packs.len() => {
                Some(idx + 1)
            }
            _ => None,
        };

        let Some(target_idx) = target else {
            return;
        };

        if !packs[idx].enabled || !packs[target_idx].enabled {
            return;
        }
        packs.swap(idx, target_idx);
    }

    fn manage_pack_target(&self, selected: usize) -> Option<(bool, usize)> {
        if self.servers.is_empty() {
            return None;
        }
        let server = &self.servers[self.selected];
        let rp_len = server.installed_resource_packs.len();
        let bp_len = server.installed_behavior_packs.len();

        let mut current = 0;
        current += 1; // resource header

        if rp_len == 0 {
            if selected == current {
                return None;
            }
            current += 1;
        } else {
            for i in 0..rp_len {
                if selected == current {
                    return Some((true, i));
                }
                current += 1;
            }
        }

        if selected == current {
            return None; // separator
        }
        current += 1;

        if selected == current {
            return None; // behavior header
        }
        current += 1;

        if bp_len == 0 {
            if selected == current {
                return None;
            }
        } else {
            for i in 0..bp_len {
                if selected == current {
                    return Some((false, i));
                }
                current += 1;
            }
        }
        None
    }

    fn toggle_pack_by_index(&mut self, is_resource: bool, idx: usize) {
        let server = &self.servers[self.selected];

        let (uuid, version, currently_enabled) = if is_resource {
            let pack = &server.installed_resource_packs[idx];
            (pack.uuid.clone(), pack.version.clone(), pack.enabled)
        } else {
            let pack = &server.installed_behavior_packs[idx];
            (pack.uuid.clone(), pack.version.clone(), pack.enabled)
        };

        match crate::plugin::installer::set_pack_enabled(
            &server.path,
            &uuid,
            &version,
            is_resource,
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

        self.run_auto_refresh();
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
            AppEvent::InstallPlugin(path, custom_name) => self.run_install(path, custom_name),
            AppEvent::InstallDone(result) => match result {
                Ok(msg) => {
                    self.message = Some(msg);
                    self.run_auto_refresh();
                }
                Err(err) => {
                    self.message = Some(format!("Install error: {err}"));
                }
            },
            AppEvent::ImportWorld(path, mode, target_world) => {
                self.run_world_import(path, mode, target_world)
            }
            AppEvent::ImportWorldDone(result) => match result {
                Ok(msg) => {
                    self.message = Some(msg);
                    self.run_auto_refresh();
                }
                Err(err) => {
                    self.message = Some(format!("World import error: {err}"));
                }
            },
            AppEvent::UpdateStatuses => self.run_update_statuses(),
            AppEvent::StatusesUpdated(updates) => {
                self.status_refresh_pending = false;
                for update in updates {
                    if let Some(server) = self.servers.iter_mut().find(|s| s.path == update.path) {
                        server.apply_status_update(update);
                    }
                }
            }
            AppEvent::LogsLoaded(lines) => {
                self.log_lines = lines;
                self.message = None;
                if let AppMode::ViewLogs { scroll } = &mut self.mode {
                    *scroll = self.log_lines.len().saturating_sub(1);
                }
            }
            AppEvent::ServersRefreshed(servers) => {
                self.servers = servers;
                self.selected = self.selected.min(self.servers.len().saturating_sub(1));
            }
            AppEvent::CommandSent(result) => match result {
                Ok(msg) => self.message = Some(msg),
                Err(err) => self.message = Some(format!("Command error: {err}")),
            },
            AppEvent::ServerRestarted(result) => {
                match result {
                    Ok(msg) => self.message = Some(msg),
                    Err(err) => self.message = Some(format!("Restart error: {err}")),
                }
                // Refresh after a restart attempt
                self.run_auto_refresh();
            }
        }
    }

    fn run_auto_refresh(&self) {
        let connections = self.connections.clone();
        let servers_path = self.servers_path.clone();
        let sender = self.events.sender();
        tokio::spawn(async move {
            let servers = tokio::task::spawn_blocking(move || {
                discover_servers_with_connections(&connections, &servers_path)
            })
            .await
            .unwrap_or_default();
            let _ = sender.send(Event::App(AppEvent::ServersRefreshed(servers)));
        });
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

    /// Spawn one `spawn_blocking` task per server, all running concurrently.
    /// Results come back as `AppEvent::StatusesUpdated` so the event loop is never blocked.
    fn run_update_statuses(&mut self) {
        if self.status_refresh_pending {
            return;
        }
        self.status_refresh_pending = true;

        let inputs: Vec<StatusRefreshInput> = self
            .servers
            .iter()
            .map(|s| StatusRefreshInput {
                path: s.path.clone(),
                server_type: s.server_type.clone(),
                port: s.port,
                container_name: s.container_name.clone(),
                prev_cpu_sample: s.cpu_sample.clone(),
            })
            .collect();

        let sender = self.events.sender();
        tokio::spawn(async move {
            // Spawn all servers in parallel on the blocking thread-pool.
            let handles: Vec<_> = inputs
                .into_iter()
                .map(|input| tokio::task::spawn_blocking(move || compute_status_update(input)))
                .collect();

            let mut updates = Vec::with_capacity(handles.len());
            for handle in handles {
                if let Ok(update) = handle.await {
                    updates.push(update);
                }
            }
            let _ = sender.send(Event::App(AppEvent::StatusesUpdated(updates)));
        });
    }

    fn run_install(&self, path: PathBuf, custom_name: Option<String>) {
        let server_path = self.servers[self.selected].path.clone();
        let sender = self.events.sender();
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                crate::plugin::installer::install(&path, &server_path, custom_name)
                    .map(|summary| {
                        let names = summary
                            .installed
                            .iter()
                            .map(|r| format!("'{}' ({})", r.pack_name, r.pack_type))
                            .collect::<Vec<_>>()
                            .join(", ");
                        if summary.skipped_errors.is_empty() {
                            format!("Installed: {names}")
                        } else {
                            format!(
                                "Installed: {names} | Skipped: {}",
                                summary.skipped_errors.join(" | ")
                            )
                        }
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

    fn run_world_import(
        &self,
        path: PathBuf,
        mode: crate::world::WorldImportMode,
        target_world: Option<String>,
    ) {
        let server_path = self.servers[self.selected].path.clone();
        let sender = self.events.sender();
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                crate::world::import_mcworld(&server_path, &path, mode, target_world)
                    .map_err(|e| e.to_string())
            })
            .await;
            let msg = match result {
                Ok(r) => r,
                Err(e) => Err(e.to_string()),
            };
            let _ = sender.send(Event::App(AppEvent::ImportWorldDone(msg)));
        });
    }
}

fn is_mcworld_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mcworld"))
}

fn discover_servers_with_connections(
    connections: &ConnectionConfig,
    legacy_path: &Path,
) -> Vec<ServerInstance> {
    let mut servers = Vec::new();

    for conn in &connections.connections {
        if conn.path.exists() {
            let server = ServerInstance::from_path_with_container(
                &conn.path,
                Some(&conn.name),
                conn.container_name.as_deref(),
            );
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
