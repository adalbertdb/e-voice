//! System tray integration and daemon orchestration for desktop mode switching.

use crate::config::{AppConfig, config_file_path};
use crate::daemon::{Daemon, Request, handle_request};
use std::process::Command;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::Duration;
use thiserror::Error;
use tokio::runtime::Builder;
use tokio::sync::{mpsc, watch};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

const MODE_LABELS: [(&str, &str); 5] = [
    ("clean", "Clean"),
    ("formal", "Formal"),
    ("casual", "Casual"),
    ("bullet", "Bullet"),
    ("translate:en", "Translate (EN)"),
];

#[derive(Debug, Clone)]
pub enum DaemonControl {
    SetMode(String),
    Shutdown,
}

#[derive(Debug)]
pub struct DaemonRuntime {
    pub control_tx: mpsc::UnboundedSender<DaemonControl>,
    pub mode_rx: Receiver<String>,
    pub shutdown_rx: Receiver<()>,
    pub initial_mode: String,
}

#[derive(Debug, Error)]
pub enum TrayError {
    #[error("failed to start daemon runtime: {0}")]
    RuntimeStart(String),
    #[error("tray icon error: {0}")]
    TrayIcon(#[from] tray_icon::Error),
    #[error("invalid tray icon data: {0}")]
    BadIcon(#[from] tray_icon::BadIcon),
    #[error("menu error: {0}")]
    Menu(#[from] tray_icon::menu::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to open config path")]
    ConfigPath,
}

pub fn start_daemon_runtime() -> Result<DaemonRuntime, TrayError> {
    let (control_tx, control_rx) = mpsc::unbounded_channel::<DaemonControl>();
    let (mode_tx, mode_rx) = channel::<String>();
    let (shutdown_tx, shutdown_rx) = channel::<()>();
    let (startup_tx, startup_rx) = channel::<Result<String, String>>();

    thread::spawn(move || {
        let runtime = match Builder::new_multi_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(err) => {
                let _ = startup_tx.send(Err(format!("failed to create tokio runtime: {err}")));
                return;
            }
        };

        runtime.block_on(async move {
            let config = match AppConfig::load() {
                Ok(cfg) => cfg,
                Err(err) => {
                    let _ = startup_tx.send(Err(format!("failed to load config: {err}")));
                    return;
                }
            };

            let daemon = match Daemon::new(config) {
                Ok(d) => d,
                Err(err) => {
                    let _ = startup_tx.send(Err(format!("failed to initialize daemon: {err}")));
                    return;
                }
            };

            let state = daemon.shared_state();
            let initial_mode = match state.lock() {
                Ok(guard) => guard.mode().to_string(),
                Err(_) => "clean".to_owned(),
            };
            let _ = startup_tx.send(Ok(initial_mode));

            let (shutdown_signal_tx, shutdown_signal_rx) = watch::channel(false);

            let daemon_task = tokio::spawn(async move { daemon.run(shutdown_signal_rx).await });

            let mode_sender_task = {
                let state = state.clone();
                let mode_tx = mode_tx.clone();
                let mut stop = shutdown_signal_tx.subscribe();
                tokio::spawn(async move {
                    loop {
                        if *stop.borrow() {
                            break;
                        }

                        if let Ok(guard) = state.lock() {
                            let _ = mode_tx.send(guard.mode().to_string());
                        }

                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                            changed = stop.changed() => {
                                if changed.is_ok() && *stop.borrow() {
                                    break;
                                }
                            }
                        }
                    }
                })
            };

            let signal_task = {
                let shutdown_signal = shutdown_signal_tx.clone();
                let shutdown_notice = shutdown_tx.clone();
                tokio::spawn(async move {
                    #[cfg(unix)]
                    {
                        let mut sigterm = tokio::signal::unix::signal(
                            tokio::signal::unix::SignalKind::terminate(),
                        )
                        .ok();

                        tokio::select! {
                            _ = tokio::signal::ctrl_c() => {}
                            _ = async {
                                if let Some(sig) = sigterm.as_mut() {
                                    let _ = sig.recv().await;
                                }
                            } => {}
                        }
                    }

                    #[cfg(not(unix))]
                    {
                        let _ = tokio::signal::ctrl_c().await;
                    }

                    let _ = shutdown_signal.send(true);
                    let _ = shutdown_notice.send(());
                })
            };

            process_control_messages(
                control_rx,
                state,
                shutdown_signal_tx.clone(),
                shutdown_signal_tx.subscribe(),
            )
            .await;

            let _ = shutdown_signal_tx.send(true);
            let _ = signal_task.await;
            let _ = mode_sender_task.await;
            let _ = daemon_task.await;
            let _ = shutdown_tx.send(());
        });
    });

    let initial_mode = startup_rx
        .recv()
        .map_err(|_| TrayError::RuntimeStart("runtime did not report startup status".to_owned()))?
        .map_err(TrayError::RuntimeStart)?;

    Ok(DaemonRuntime {
        control_tx,
        mode_rx,
        shutdown_rx,
        initial_mode,
    })
}

async fn process_control_messages(
    mut control_rx: mpsc::UnboundedReceiver<DaemonControl>,
    state: crate::daemon::SharedState,
    shutdown_tx: watch::Sender<bool>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            maybe_command = control_rx.recv() => {
                match maybe_command {
                    Some(DaemonControl::SetMode(mode)) => {
                        let _ = handle_request(Request::SetMode { mode }, state.clone()).await;
                    }
                    Some(DaemonControl::Shutdown) => {
                        let _ = shutdown_tx.send(true);
                        break;
                    }
                    None => break,
                }
            }
            changed = shutdown_rx.changed() => {
                if changed.is_ok() && *shutdown_rx.borrow() {
                    break;
                }
            }
        }
    }
}

pub fn run_tray_loop(runtime: DaemonRuntime) -> Result<(), TrayError> {
    let mut tray = match TrayUi::new(runtime.control_tx.clone(), &runtime.initial_mode) {
        Ok(ui) => Some(ui),
        Err(err) => {
            eprintln!("tray unavailable on this compositor/session: {err}");
            None
        }
    };

    loop {
        if runtime.shutdown_rx.try_recv().is_ok() {
            break;
        }

        if let Ok(mode) = runtime.mode_rx.try_recv()
            && let Some(ui) = tray.as_mut()
        {
            ui.set_mode(&mode);
        }

        if let Some(ui) = tray.as_mut()
            && ui.handle_events()?
        {
            break;
        }

        thread::sleep(Duration::from_millis(100));
    }

    if let Some(ui) = tray.take() {
        ui.shutdown();
    }

    let _ = runtime.control_tx.send(DaemonControl::Shutdown);
    Ok(())
}

struct TrayUi {
    tray_icon: TrayIcon,
    menu_mode_items: Vec<(String, MenuItem, String)>,
    open_config: MenuItem,
    quit_item: MenuItem,
    control_tx: mpsc::UnboundedSender<DaemonControl>,
    current_mode: String,
}

impl TrayUi {
    fn new(
        control_tx: mpsc::UnboundedSender<DaemonControl>,
        initial_mode: &str,
    ) -> Result<Self, TrayError> {
        let display_set = std::env::var("DISPLAY").map(|s| !s.is_empty()).unwrap_or(false);
        let wayland_set = std::env::var("WAYLAND_DISPLAY").map(|s| !s.is_empty()).unwrap_or(false);
        if !display_set && !wayland_set {
            return Err(TrayError::RuntimeStart(
                "no display available".to_owned(),
            ));
        }

        let menu = Menu::new();

        let mut menu_mode_items = Vec::new();
        for (value, label) in MODE_LABELS {
            let item = MenuItem::with_id(format!("mode:{value}"), label, true, None);
            menu.append(&item)?;
            menu_mode_items.push((value.to_owned(), item, label.to_owned()));
        }

        let separator = PredefinedMenuItem::separator();
        menu.append(&separator)?;

        let open_config = MenuItem::with_id("open-config", "Open Config", true, None);
        menu.append(&open_config)?;

        let quit_item = MenuItem::with_id("quit", "Quit", true, None);
        menu.append(&quit_item)?;

        let tray_icon = TrayIconBuilder::new()
            .with_icon(build_icon()?)
            .with_tooltip(tooltip_for_mode(initial_mode))
            .with_title(mode_title(initial_mode))
            .with_menu(Box::new(menu))
            .build()?;

        let mut this = Self {
            tray_icon,
            menu_mode_items,
            open_config,
            quit_item,
            control_tx,
            current_mode: initial_mode.to_owned(),
        };

        this.render_menu_labels();
        Ok(this)
    }

    fn set_mode(&mut self, mode: &str) {
        if self.current_mode == mode {
            return;
        }
        self.current_mode = mode.to_owned();
        self.render_menu_labels();
    }

    fn render_menu_labels(&mut self) {
        let _ = self
            .tray_icon
            .set_tooltip(Some(tooltip_for_mode(&self.current_mode)));
        self.tray_icon
            .set_title(Some(mode_title(&self.current_mode)));

        for (value, item, base_label) in &self.menu_mode_items {
            if *value == self.current_mode {
                item.set_text(format!("✓ {base_label}"));
            } else {
                item.set_text(format!("  {base_label}"));
            }
        }
    }

    fn handle_events(&mut self) -> Result<bool, TrayError> {
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button,
                button_state,
                ..
            } = event
                && button == MouseButton::Left
                && button_state == MouseButtonState::Up
            {
                let next_mode = next_mode(&self.current_mode);
                self.current_mode = next_mode.to_owned();
                self.render_menu_labels();
                let _ = self
                    .control_tx
                    .send(DaemonControl::SetMode(next_mode.to_owned()));
            }
        }

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.quit_item.id().clone() {
                let _ = self.control_tx.send(DaemonControl::Shutdown);
                return Ok(true);
            }

            if event.id == self.open_config.id().clone() {
                open_config_file()?;
            }

            let mut selected_mode: Option<String> = None;
            for (mode, item, _) in &self.menu_mode_items {
                if event.id == item.id().clone() {
                    selected_mode = Some(mode.clone());
                    break;
                }
            }

            if let Some(mode) = selected_mode {
                self.current_mode = mode.clone();
                self.render_menu_labels();
                let _ = self.control_tx.send(DaemonControl::SetMode(mode));
            }
        }

        Ok(false)
    }

    fn shutdown(self) {
        drop(self.tray_icon);
    }
}

fn open_config_file() -> Result<(), TrayError> {
    let config_path = config_file_path().map_err(|_| TrayError::ConfigPath)?;
    let status = Command::new("xdg-open").arg(config_path).status()?;
    if !status.success() {
        return Err(TrayError::Io(std::io::Error::other(
            "xdg-open returned non-zero status",
        )));
    }
    Ok(())
}

fn mode_title(mode: &str) -> String {
    format!("e-voice: {mode}")
}

fn tooltip_for_mode(mode: &str) -> String {
    format!("e-voice active | mode: {mode}")
}

fn next_mode(current_mode: &str) -> &'static str {
    match current_mode {
        "clean" => "formal",
        "formal" => "casual",
        "casual" => "bullet",
        "bullet" => "translate:en",
        _ => "clean",
    }
}

fn build_icon() -> Result<Icon, TrayError> {
    let mut rgba = Vec::with_capacity(16 * 16 * 4);
    for _ in 0..(16 * 16) {
        rgba.extend_from_slice(&[0x29, 0x7a, 0xff, 0xff]);
    }
    Ok(Icon::from_rgba(rgba, 16, 16)?)
}
