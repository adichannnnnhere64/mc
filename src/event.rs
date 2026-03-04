use color_eyre::eyre::OptionExt;
use crossterm::event::Event as CrosstermEvent;
use futures::{FutureExt, StreamExt};
use std::{path::PathBuf, time::Duration};
use tokio::sync::mpsc;

use crate::server::ServerInstance;

const TICK_FPS: f64 = 30.0;

#[derive(Clone, Debug)]
pub enum Event {
    Tick,
    Crossterm(CrosstermEvent),
    App(AppEvent),
}

#[derive(Clone, Debug)]
pub enum AppEvent {
    Quit,
    SelectNext,
    SelectPrev,
    InstallPlugin(PathBuf, Option<String>), // path, optional custom name
    InstallDone(Result<String, String>),
    LogsLoaded(Vec<String>),
    ServersRefreshed(Vec<ServerInstance>),
    CommandSent(Result<String, String>),
    ServerRestarted(Result<String, String>),
    UpdateStatuses,
}

#[derive(Debug)]
pub struct EventHandler {
    sender: mpsc::UnboundedSender<Event>,
    receiver: mpsc::UnboundedReceiver<Event>,
}

impl EventHandler {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let actor = EventTask::new(sender.clone());
        tokio::spawn(async { actor.run().await });
        Self { sender, receiver }
    }

    pub async fn next(&mut self) -> color_eyre::Result<Event> {
        self.receiver
            .recv()
            .await
            .ok_or_eyre("Event channel closed unexpectedly")
    }

    pub fn send(&self, app_event: AppEvent) {
        let _ = self.sender.send(Event::App(app_event));
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<Event> {
        self.sender.clone()
    }
}

struct EventTask {
    sender: mpsc::UnboundedSender<Event>,
}

impl EventTask {
    fn new(sender: mpsc::UnboundedSender<Event>) -> Self {
        Self { sender }
    }

    async fn run(self) -> color_eyre::Result<()> {
        let tick_rate = Duration::from_secs_f64(1.0 / TICK_FPS);
        let mut reader = crossterm::event::EventStream::new();
        let mut tick = tokio::time::interval(tick_rate);

        loop {
            let tick_delay = tick.tick();
            let crossterm_event = reader.next().fuse();

            tokio::select! {
                _ = self.sender.closed() => break,
                _ = tick_delay => self.send(Event::Tick),
                Some(Ok(evt)) = crossterm_event => self.send(Event::Crossterm(evt)),

            }
        }
        Ok(())
    }

    fn send(&self, event: Event) {
        let _ = self.sender.send(event);
    }
}
