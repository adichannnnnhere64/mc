use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::{
    event::{AppEvent, Event, EventHandler},
    server::{ServerInstance, discover_servers},
    ui,
};

#[derive(Debug)]
pub enum AppMode {
    Normal,
    Installing { input: String },
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
}

impl App {
    pub fn new() -> Self {
        let servers_path = PathBuf::from("servers");
        let servers = discover_servers(&servers_path);
        Self {
            running: true,
            servers,
            selected: 0,
            mode: AppMode::Normal,
            message: None,
            servers_path,
            events: EventHandler::new(),
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
                        Some("No servers found. Create ./servers/<name>/ to get started.".into());
                }
            }
            KeyCode::Char('r') => {
                self.servers = discover_servers(&self.servers_path);
                self.selected = self.selected.min(self.servers.len().saturating_sub(1));
                self.message = Some("Server list refreshed.".into());
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
                    self.servers = discover_servers(&self.servers_path);
                    self.selected = self.selected.min(self.servers.len().saturating_sub(1));
                }
                Err(err) => {
                    self.message = Some(format!("Install error: {err}"));
                }
            },
        }
    }

    fn run_install(&self, path: PathBuf) {
        let server_path = self.servers[self.selected].path.clone();
        let sender = self.events.sender();
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                crate::plugin::installer::install(&path, &server_path)
                    .map(|r| format!("Installed '{}' ({})", r.pack_name, r.pack_type))
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
