# minecraft-plugin-installer

A lightweight terminal-based Minecraft server manager built in Rust using `ratatui` + `crossterm`.

## Purpose

- Manage multiple Minecraft server instances
- View server status and installed packs in a structured TUI
- Install plugins/addons from folders, `.zip`, or `.mcaddon` files
- Auto-update `resource_packs.json` and `behavior_packs.json`

## Core Principles

- **Lightweight first** — prefer `std`, avoid heavy crates, minimal allocations
- **No dynamic dispatch** — avoid `Box<dyn Trait>` unless strictly necessary
- **Async-safe** — Tokio runtime; file I/O in `spawn_blocking`, UI thread never blocks
- **Never panic** at runtime — use `color-eyre` for error propagation

## Architecture

```
src/
├── main.rs          → Entry point, tokio runtime
├── app.rs           → App state, main loop, event handling
├── event.rs         → Event types and handler (crossterm + app events)
├── ui.rs            → Rendering logic (ratatui)
├── server/
│   └── mod.rs       → ServerInstance, ServerStatus, pack discovery
└── plugin/
    ├── mod.rs
    ├── manifest.rs  → Deserialize manifest.json
    ├── extractor.rs → Zip/mcaddon extraction
    └── installer.rs → High-level install flow
```

## Key Data Structures

```rust
pub struct App {
    pub running: bool,
    pub servers: Vec<ServerInstance>,
    pub selected: usize,
    pub mode: AppMode,
    pub message: Option<String>,
    pub servers_path: PathBuf,
    pub events: EventHandler,
}

pub enum AppMode {
    Normal,
    Installing { input: String },
}

pub struct ServerInstance {
    pub name: String,
    pub path: PathBuf,
    pub status: ServerStatus,
    pub resource_packs: Vec<PackEntry>,
    pub behavior_packs: Vec<PackEntry>,
}

pub enum ServerStatus { Running, Stopped, Starting, Error(String) }
```

## Server Directory Layout

Servers are discovered from `./servers/` (relative to CWD):

```
servers/
└── my-server/
    ├── server.properties
    ├── resource_packs.json
    ├── behavior_packs.json
    ├── resource_packs/
    └── behavior_packs/
```

## UI Layout

Fixed 30% / 70% horizontal split with a 1-line status bar at the bottom:

```
┌─ Servers (2) ────────┬─ Server Details ──────────────────────┐
│  ● my-server RUNNING │   Name:    my-server                   │
│  ○ survival  STOPPED │   Path:    ./servers/my-server         │
│                      │   Status:  RUNNING                     │
│                      │                                        │
│                      │   Resource Packs (1):                  │
│                      │     • <uuid>  v1.0.0                   │
│                      │                                        │
│                      │   Behavior Packs (0):                  │
│                      │     (none)                             │
└──────────────────────┴────────────────────────────────────────┘
  q quit   ↑↓ navigate   i install   r refresh
```

Install modal overlay appears on `i`:

```
        ┌─ Install Plugin ──────────────────┐
        │                                    │
        │  Path: /path/to/addon.mcaddon█    │
        │                                    │
        │  Supports: folder, .zip, .mcaddon  │
        └────────────────────────────────────┘
           Enter confirm   Esc cancel
```

## Key Bindings

| Key      | Action                    |
|----------|---------------------------|
| `↑` / `↓` | Navigate server list    |
| `i`      | Open install plugin modal |
| `r`      | Refresh server list       |
| `q` / `Esc` | Quit                   |
| `Ctrl-C` | Quit                      |

Modal:

| Key     | Action  |
|---------|---------|
| `Enter` | Confirm |
| `Esc`   | Cancel  |

## Plugin Installation Flow

1. User presses `i`, types a path, presses Enter
2. Path is sent as `AppEvent::InstallPlugin(PathBuf)` via channel
3. `tokio::task::spawn_blocking` runs synchronous install logic
4. On completion, `AppEvent::InstallDone(Result<String, String>)` is sent back
5. UI shows success/error message; server packs are refreshed

Install logic:
- If folder: use as-is
- If `.zip`/`.mcaddon`: extract to `.tmp_install/` in server dir
- Find `manifest.json` recursively
- Parse pack type from `modules[].type` (`"resources"` → resource, `"data"` → behavior)
- Copy pack directory to `resource_packs/<uuid>/` or `behavior_packs/<uuid>/`
- Update `resource_packs.json` or `behavior_packs.json` (backup `.bak` first, deduplicate by UUID)
- Clean up temp dir

## AppEvent Enum

```rust
pub enum AppEvent {
    Quit,
    SelectNext,
    SelectPrev,
    InstallPlugin(PathBuf),
    InstallDone(Result<String, String>),
}
```

## Cargo.toml Optimization

```toml
[profile.release]
codegen-units = 1
lto = true
opt-level = "s"
strip = true
```

Avoid default features on heavy crates. Use `zip` with `deflate` only.

## Error Handling

- Use `color-eyre` throughout
- Never `unwrap()` or `panic!()` at runtime
- Surface errors as `app.message` displayed in the status bar

## Non-Goals

- Web UI or heavy GUI frameworks
- Plugin marketplace
- Remote/SSH server management
- Database storage
- Overengineered abstractions
