use crate::autostart;
use crate::cli;
use crate::config::{AlbumMode, Config, FolderConfig};
use crate::process::{LogEvent, WatcherHandle, run_sync_now, spawn_watcher};
use crate::tray::TrayState;
use eframe::egui;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tray_icon::TrayIconEvent;
use tray_icon::menu::MenuEvent;

const LOG_LIMIT: usize = 5_000;

#[derive(Debug, Clone, PartialEq, Eq)]
enum FolderState {
    Idle,
    Watching,
    Stopping,
    Error,
}

impl FolderState {
    fn label(&self) -> &'static str {
        match self {
            FolderState::Idle => "● Idle",
            FolderState::Watching => "● Watching",
            FolderState::Stopping => "● Stopping",
            FolderState::Error => "● Error",
        }
    }

    fn color(&self) -> egui::Color32 {
        match self {
            FolderState::Idle => egui::Color32::GRAY,
            FolderState::Watching => egui::Color32::from_rgb(80, 200, 120),
            FolderState::Stopping => egui::Color32::YELLOW,
            FolderState::Error => egui::Color32::from_rgb(220, 80, 80),
        }
    }
}

struct LogBuffer {
    completed: VecDeque<String>,
    partial: Option<String>,
}

impl LogBuffer {
    fn new() -> Self {
        Self {
            completed: VecDeque::new(),
            partial: None,
        }
    }

    fn push_line(&mut self, line: String, partial: bool) {
        if partial {
            self.partial = Some(line);
        } else {
            self.partial = None;
            if self.completed.len() >= LOG_LIMIT {
                self.completed.pop_front();
            }
            self.completed.push_back(line);
        }
    }

    fn push_system(&mut self, line: String) {
        if self.completed.len() >= LOG_LIMIT {
            self.completed.pop_front();
        }
        self.completed.push_back(line);
    }

    fn clear(&mut self) {
        self.completed.clear();
        self.partial = None;
    }

    fn iter_display<'a>(&'a self) -> impl Iterator<Item = &'a str> {
        self.completed
            .iter()
            .map(String::as_str)
            .chain(self.partial.as_deref())
    }
}

#[derive(Clone, PartialEq, Eq)]
enum LogTab {
    All,
    Folder(String),
}

pub struct App {
    config: Config,
    rt: Handle,
    log_tx: UnboundedSender<LogEvent>,
    log_rx: UnboundedReceiver<LogEvent>,

    api_key_input: String,
    server_url_input: String,

    cli_status: Arc<Mutex<CliStatus>>,
    login_status: Arc<Mutex<LoginStatus>>,

    folder_states: HashMap<String, FolderState>,
    folder_logs: HashMap<String, LogBuffer>,
    all_log: LogBuffer,
    watchers: HashMap<String, WatcherHandle>,

    selected_tab: LogTab,
    edit_folder: Option<String>,

    tray: Option<TrayState>,
    tray_pause_all_active: bool,

    pending_save: bool,

    quit_requested: bool,
    minimized_to_tray: bool,
}

#[derive(Clone)]
enum CliStatus {
    Unknown,
    Checking,
    Ok(String),
    Missing(String),
}

#[derive(Clone)]
enum LoginStatus {
    Idle,
    Running,
    Ok(String),
    Failed(String),
}

impl App {
    pub fn new(
        config: Config,
        warning: Option<String>,
        rt: Handle,
        tray: Option<TrayState>,
    ) -> Self {
        let (log_tx, log_rx) = unbounded_channel();
        let cli_status = Arc::new(Mutex::new(CliStatus::Unknown));
        let login_status = Arc::new(Mutex::new(LoginStatus::Idle));

        let mut folder_logs: HashMap<String, LogBuffer> = HashMap::new();
        let mut folder_states: HashMap<String, FolderState> = HashMap::new();
        for f in &config.folders {
            folder_logs.insert(f.id.clone(), LogBuffer::new());
            folder_states.insert(f.id.clone(), FolderState::Idle);
        }

        let server_url_input = config.server_url.clone();

        let mut app = Self {
            config,
            rt,
            log_tx,
            log_rx,
            api_key_input: String::new(),
            server_url_input,
            cli_status,
            login_status,
            folder_states,
            folder_logs,
            all_log: LogBuffer::new(),
            watchers: HashMap::new(),
            selected_tab: LogTab::All,
            edit_folder: None,
            tray,
            tray_pause_all_active: false,
            pending_save: false,
            quit_requested: false,
            minimized_to_tray: false,
        };

        if let Some(w) = warning {
            app.all_log.push_system(format!("[warn] {w}"));
        }

        app.kick_cli_check();
        app.start_all_enabled_watchers();

        app
    }

    fn kick_cli_check(&self) {
        let status = self.cli_status.clone();
        *status.lock().unwrap() = CliStatus::Checking;
        self.rt.spawn(async move {
            let result = cli::check_immich_installed().await;
            let mut s = status.lock().unwrap();
            *s = match result {
                Ok(v) => CliStatus::Ok(v),
                Err(e) => CliStatus::Missing(e.to_string()),
            };
        });
    }

    fn kick_login(&self) {
        let url = self.server_url_input.trim().to_string();
        let key = self.api_key_input.trim().to_string();
        let status = self.login_status.clone();
        *status.lock().unwrap() = LoginStatus::Running;
        self.rt.spawn(async move {
            let result = cli::login(&url, &key).await;
            let mut s = status.lock().unwrap();
            *s = match result {
                Ok(msg) => LoginStatus::Ok(if msg.is_empty() {
                    "ログイン成功".into()
                } else {
                    msg
                }),
                Err(e) => LoginStatus::Failed(e.to_string()),
            };
        });
    }

    fn start_all_enabled_watchers(&mut self) {
        if !self.config.auto_start_watching_on_launch {
            return;
        }
        let folders: Vec<FolderConfig> = self
            .config
            .folders
            .iter()
            .filter(|f| f.enabled)
            .cloned()
            .collect();
        for folder in folders {
            self.start_watcher(&folder.id);
        }
    }

    fn start_watcher(&mut self, folder_id: &str) {
        if self.watchers.contains_key(folder_id) {
            return;
        }
        let Some(folder) = self.config.folders.iter().find(|f| f.id == folder_id).cloned() else {
            return;
        };
        let auto_restart = self.config.auto_restart_on_failure;
        let max_attempts = self.config.max_restart_attempts.max(1);
        let backoff = self.config.restart_backoff_seconds;
        let _guard = self.rt.enter();
        let handle = spawn_watcher(folder, self.log_tx.clone(), auto_restart, max_attempts, backoff);
        self.folder_states
            .insert(folder_id.to_string(), FolderState::Watching);
        self.watchers.insert(folder_id.to_string(), handle);
    }

    fn stop_watcher(&mut self, folder_id: &str) {
        if let Some(mut h) = self.watchers.remove(folder_id) {
            h.request_stop();
            self.folder_states
                .insert(folder_id.to_string(), FolderState::Stopping);
            let join_handle = h;
            self.rt.spawn(async move {
                join_handle.stop().await;
            });
        }
    }

    fn sync_now(&self, folder_id: &str) {
        let Some(folder) = self.config.folders.iter().find(|f| f.id == folder_id).cloned() else {
            return;
        };
        let log_tx = self.log_tx.clone();
        self.rt.spawn(async move {
            let folder_id = folder.id.clone();
            if let Err(e) = run_sync_now(folder, log_tx.clone()).await {
                let _ = log_tx.send(LogEvent::Error {
                    folder_id,
                    message: format!("Sync Now 失敗: {e}"),
                });
            }
        });
    }

    fn sync_all(&self) {
        for folder in self.config.folders.iter().filter(|f| f.enabled) {
            self.sync_now(&folder.id);
        }
    }

    fn pause_all(&mut self) {
        let ids: Vec<String> = self.watchers.keys().cloned().collect();
        for id in ids {
            self.stop_watcher(&id);
        }
        self.tray_pause_all_active = true;
    }

    fn resume_all(&mut self) {
        let ids: Vec<String> = self
            .config
            .folders
            .iter()
            .filter(|f| f.enabled)
            .map(|f| f.id.clone())
            .collect();
        for id in ids {
            self.start_watcher(&id);
        }
        self.tray_pause_all_active = false;
    }

    fn drain_log_events(&mut self, ctx: &egui::Context) {
        let mut had_event = false;
        while let Ok(ev) = self.log_rx.try_recv() {
            had_event = true;
            self.handle_log_event(ev);
        }
        if had_event {
            ctx.request_repaint();
        }
    }

    fn handle_log_event(&mut self, ev: LogEvent) {
        match ev {
            LogEvent::Line {
                folder_id,
                line,
                partial,
            } => {
                let label = self.folder_label(&folder_id);
                let display = format!("[{label}] {line}");
                if let Some(buf) = self.folder_logs.get_mut(&folder_id) {
                    buf.push_line(line.clone(), partial);
                }
                if !partial {
                    self.all_log.push_line(display, false);
                }
            }
            LogEvent::Started { folder_id, pid } => {
                let label = self.folder_label(&folder_id);
                let msg = format!("[{label}] [system] watcher started (pid={pid})");
                if let Some(buf) = self.folder_logs.get_mut(&folder_id) {
                    buf.push_system(format!("[system] watcher started (pid={pid})"));
                }
                self.all_log.push_system(msg);
                self.folder_states
                    .insert(folder_id, FolderState::Watching);
            }
            LogEvent::Exited {
                folder_id,
                code,
                was_canceled,
            } => {
                let label = self.folder_label(&folder_id);
                let msg = format!(
                    "[{label}] [system] exit code={:?} canceled={was_canceled}",
                    code
                );
                if let Some(buf) = self.folder_logs.get_mut(&folder_id) {
                    buf.push_system(format!(
                        "[system] exit code={:?} canceled={was_canceled}",
                        code
                    ));
                }
                self.all_log.push_system(msg);
                self.watchers.remove(&folder_id);
                if was_canceled {
                    self.folder_states.insert(folder_id, FolderState::Idle);
                } else if matches!(code, Some(0)) {
                    self.folder_states.insert(folder_id, FolderState::Idle);
                } else {
                    self.folder_states.insert(folder_id, FolderState::Error);
                }
            }
            LogEvent::Error { folder_id, message } => {
                let label = self.folder_label(&folder_id);
                let msg = format!("[{label}] [error] {message}");
                if let Some(buf) = self.folder_logs.get_mut(&folder_id) {
                    buf.push_system(format!("[error] {message}"));
                }
                self.all_log.push_system(msg);
                self.folder_states.insert(folder_id, FolderState::Error);
            }
        }
    }

    fn folder_label(&self, folder_id: &str) -> String {
        self.config
            .folders
            .iter()
            .find(|f| f.id == folder_id)
            .map(|f| {
                f.path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| f.path.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| folder_id.to_string())
    }

    fn add_folder(&mut self, path: PathBuf) {
        let folder = FolderConfig::new(path);
        self.folder_logs.insert(folder.id.clone(), LogBuffer::new());
        self.folder_states
            .insert(folder.id.clone(), FolderState::Idle);
        self.config.folders.push(folder);
        self.pending_save = true;
    }

    fn remove_folder(&mut self, folder_id: &str) {
        self.stop_watcher(folder_id);
        self.config.folders.retain(|f| f.id != folder_id);
        self.folder_states.remove(folder_id);
        self.folder_logs.remove(folder_id);
        if let LogTab::Folder(id) = &self.selected_tab {
            if id == folder_id {
                self.selected_tab = LogTab::All;
            }
        }
        self.pending_save = true;
    }

    fn handle_tray_events(&mut self, ctx: &egui::Context) {
        let menu_rx = MenuEvent::receiver();
        while let Ok(ev) = menu_rx.try_recv() {
            let id = ev.id();
            if id == &self.tray_show_id() {
                self.show_window(ctx);
            } else if id == &self.tray_sync_all_id() {
                self.sync_all();
            } else if id == &self.tray_pause_all_id() {
                self.pause_all();
            } else if id == &self.tray_resume_all_id() {
                self.resume_all();
            } else if id == &self.tray_quit_id() {
                self.quit_requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        let tray_rx = TrayIconEvent::receiver();
        while let Ok(ev) = tray_rx.try_recv() {
            if let TrayIconEvent::DoubleClick { .. } = ev {
                self.show_window(ctx);
            }
        }
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        self.minimized_to_tray = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn tray_show_id(&self) -> tray_icon::menu::MenuId {
        self.tray
            .as_ref()
            .map(|t| t.menu_show_id.clone())
            .unwrap_or(tray_icon::menu::MenuId::new("show"))
    }
    fn tray_sync_all_id(&self) -> tray_icon::menu::MenuId {
        self.tray
            .as_ref()
            .map(|t| t.menu_sync_all_id.clone())
            .unwrap_or(tray_icon::menu::MenuId::new("sync_all"))
    }
    fn tray_pause_all_id(&self) -> tray_icon::menu::MenuId {
        self.tray
            .as_ref()
            .map(|t| t.menu_pause_all_id.clone())
            .unwrap_or(tray_icon::menu::MenuId::new("pause_all"))
    }
    fn tray_resume_all_id(&self) -> tray_icon::menu::MenuId {
        self.tray
            .as_ref()
            .map(|t| t.menu_resume_all_id.clone())
            .unwrap_or(tray_icon::menu::MenuId::new("resume_all"))
    }
    fn tray_quit_id(&self) -> tray_icon::menu::MenuId {
        self.tray
            .as_ref()
            .map(|t| t.menu_quit_id.clone())
            .unwrap_or(tray_icon::menu::MenuId::new("quit"))
    }

    fn save_if_dirty(&mut self) {
        if !self.pending_save {
            return;
        }
        self.pending_save = false;
        if let Err(e) = self.config.save() {
            self.all_log
                .push_system(format!("[warn] config 保存失敗: {e}"));
        }
    }

    fn ui_server_section(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Server")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("URL:");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.server_url_input)
                            .desired_width(360.0)
                            .hint_text("http://192.168.x.x:2283"),
                    );
                    if resp.changed() {
                        self.config.server_url = self.server_url_input.clone();
                        self.pending_save = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("API Key:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.api_key_input)
                            .password(true)
                            .desired_width(360.0)
                            .hint_text("API キー (アプリには保存されません)"),
                    );
                    let cli_ok = matches!(*self.cli_status.lock().unwrap(), CliStatus::Ok(_));
                    let login_running =
                        matches!(*self.login_status.lock().unwrap(), LoginStatus::Running);
                    let enabled = cli_ok
                        && !login_running
                        && !self.api_key_input.trim().is_empty()
                        && !self.server_url_input.trim().is_empty();
                    if ui
                        .add_enabled(enabled, egui::Button::new("Test Login"))
                        .clicked()
                    {
                        self.kick_login();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("CLI:");
                    let status = self.cli_status.lock().unwrap().clone();
                    match status {
                        CliStatus::Unknown => {
                            ui.label("未確認");
                        }
                        CliStatus::Checking => {
                            ui.spinner();
                            ui.label("確認中...");
                        }
                        CliStatus::Ok(v) => {
                            ui.colored_label(egui::Color32::from_rgb(80, 200, 120), "OK");
                            ui.label(v);
                        }
                        CliStatus::Missing(e) => {
                            ui.colored_label(egui::Color32::from_rgb(220, 80, 80), "未検出");
                            ui.label(e);
                        }
                    }
                    if ui.small_button("再チェック").clicked() {
                        self.kick_cli_check();
                    }
                });
                let login_status = self.login_status.lock().unwrap().clone();
                match login_status {
                    LoginStatus::Idle => {}
                    LoginStatus::Running => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("ログイン中...");
                        });
                    }
                    LoginStatus::Ok(msg) => {
                        ui.colored_label(
                            egui::Color32::from_rgb(80, 200, 120),
                            format!("ログイン成功: {msg}"),
                        );
                    }
                    LoginStatus::Failed(msg) => {
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 80, 80),
                            format!("ログイン失敗: {msg}"),
                        );
                    }
                }
            });
    }

    fn ui_folders_section(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Folders")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("+ Add Folder").clicked() {
                        if let Some(path) = pick_folder_dialog() {
                            self.add_folder(path);
                        }
                    }
                    ui.separator();
                    if ui.button("Sync All").clicked() {
                        self.sync_all();
                    }
                    if !self.tray_pause_all_active {
                        if ui.button("Pause All").clicked() {
                            self.pause_all();
                        }
                    } else if ui.button("Resume All").clicked() {
                        self.resume_all();
                    }
                });
                ui.add_space(4.0);

                let folder_ids: Vec<String> =
                    self.config.folders.iter().map(|f| f.id.clone()).collect();

                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for id in folder_ids {
                            self.ui_folder_row(ui, &id);
                        }
                    });
            });
    }

    fn ui_folder_row(&mut self, ui: &mut egui::Ui, folder_id: &str) {
        let mut updated_folder: Option<FolderConfig> = None;
        let mut want_remove = false;
        let mut want_sync = false;
        let mut want_start = false;
        let mut want_stop = false;
        let mut want_edit_toggle = false;

        let editing = self.edit_folder.as_deref() == Some(folder_id);
        let state = self
            .folder_states
            .get(folder_id)
            .cloned()
            .unwrap_or(FolderState::Idle);
        let watching = self.watchers.contains_key(folder_id);

        let Some(folder_ref) = self.config.folders.iter().find(|f| f.id == folder_id) else {
            return;
        };
        let mut folder = folder_ref.clone();

        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let prev_enabled = folder.enabled;
                    ui.checkbox(&mut folder.enabled, "");
                    ui.monospace(folder.path.display().to_string());
                    if folder.enabled != prev_enabled {
                        updated_folder.get_or_insert_with(|| folder.clone());
                        if !folder.enabled {
                            want_stop = true;
                        } else {
                            want_start = true;
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✕").clicked() {
                            want_remove = true;
                        }
                        if ui
                            .small_button(if editing { "Close" } else { "Edit" })
                            .clicked()
                        {
                            want_edit_toggle = true;
                        }
                        if watching {
                            if ui.small_button("Pause").clicked() {
                                want_stop = true;
                            }
                        } else if folder.enabled
                            && ui.small_button("Watch").clicked()
                        {
                            want_start = true;
                        }
                        if ui.small_button("Sync Now").clicked() {
                            want_sync = true;
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.colored_label(state.color(), state.label());
                    ui.separator();
                    let mode_label = match folder.album_mode {
                        AlbumMode::None => "Album: None".to_string(),
                        AlbumMode::FromFolder => "Album: From folder".to_string(),
                        AlbumMode::Named => format!("Album: \"{}\"", folder.album_name),
                    };
                    ui.label(mode_label);
                    ui.separator();
                    ui.label(format!(
                        "Recursive: {}  Hidden: {}  Concurrency: {}",
                        if folder.recursive { "Yes" } else { "No" },
                        if folder.include_hidden { "Yes" } else { "No" },
                        folder.concurrency,
                    ));
                });
                if editing {
                    ui.add_space(4.0);
                    let prev = folder.clone();
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut folder.recursive, "Recursive");
                        ui.checkbox(&mut folder.include_hidden, "Include hidden");
                        ui.label("Concurrency:");
                        ui.add(
                            egui::DragValue::new(&mut folder.concurrency)
                                .range(1..=64),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Album:");
                        ui.radio_value(&mut folder.album_mode, AlbumMode::None, "None");
                        ui.radio_value(
                            &mut folder.album_mode,
                            AlbumMode::FromFolder,
                            "From folder",
                        );
                        ui.radio_value(&mut folder.album_mode, AlbumMode::Named, "Named");
                        ui.add_enabled(
                            folder.album_mode == AlbumMode::Named,
                            egui::TextEdit::singleline(&mut folder.album_name)
                                .desired_width(180.0)
                                .hint_text("アルバム名"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Ignore patterns (改行区切り):");
                    });
                    let mut joined = folder.ignore_patterns.join("\n");
                    let resp = ui.add(
                        egui::TextEdit::multiline(&mut joined)
                            .desired_width(f32::INFINITY)
                            .desired_rows(2)
                            .hint_text("*.tmp\nThumbs.db"),
                    );
                    if resp.changed() {
                        folder.ignore_patterns = joined
                            .lines()
                            .map(|s| s.to_string())
                            .filter(|s| !s.trim().is_empty())
                            .collect();
                    }
                    if folder != prev {
                        updated_folder.get_or_insert_with(|| folder.clone());
                    }
                }
            });

        if let Some(updated) = updated_folder {
            if let Some(slot) = self.config.folders.iter_mut().find(|f| f.id == folder_id) {
                *slot = updated;
            }
            self.pending_save = true;
        }
        if want_edit_toggle {
            self.edit_folder = if editing {
                None
            } else {
                Some(folder_id.to_string())
            };
        }
        if want_sync {
            self.sync_now(folder_id);
        }
        if want_start {
            self.start_watcher(folder_id);
        }
        if want_stop {
            self.stop_watcher(folder_id);
        }
        if want_remove {
            self.remove_folder(folder_id);
        }
    }

    fn ui_log_section(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Log")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_label("Tab")
                        .selected_text(match &self.selected_tab {
                            LogTab::All => "All".into(),
                            LogTab::Folder(id) => self.folder_label(id),
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.selected_tab, LogTab::All, "All");
                            for f in &self.config.folders {
                                ui.selectable_value(
                                    &mut self.selected_tab,
                                    LogTab::Folder(f.id.clone()),
                                    f.path
                                        .file_name()
                                        .map(|s| s.to_string_lossy().to_string())
                                        .unwrap_or_else(|| f.path.display().to_string()),
                                );
                            }
                        });
                    if ui.button("Clear").clicked() {
                        match &self.selected_tab {
                            LogTab::All => self.all_log.clear(),
                            LogTab::Folder(id) => {
                                if let Some(b) = self.folder_logs.get_mut(id) {
                                    b.clear();
                                }
                            }
                        }
                    }
                    if ui.button("Save").clicked() {
                        self.save_log_to_file();
                    }
                });
                ui.add_space(4.0);
                let buffer: Vec<String> = match &self.selected_tab {
                    LogTab::All => self.all_log.iter_display().map(String::from).collect(),
                    LogTab::Folder(id) => self
                        .folder_logs
                        .get(id)
                        .map(|b| b.iter_display().map(String::from).collect())
                        .unwrap_or_default(),
                };
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        let text = buffer.join("\n");
                        ui.add(
                            egui::TextEdit::multiline(&mut text.as_str())
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .desired_rows(14),
                        );
                    });
            });
    }

    fn save_log_to_file(&mut self) {
        let buffer: Vec<String> = match &self.selected_tab {
            LogTab::All => self.all_log.iter_display().map(String::from).collect(),
            LogTab::Folder(id) => self
                .folder_logs
                .get(id)
                .map(|b| b.iter_display().map(String::from).collect())
                .unwrap_or_default(),
        };
        let default_name = format!(
            "immich-uploader-log-{}.txt",
            chrono_like_timestamp()
        );
        if let Some(path) = save_file_dialog(&default_name) {
            if let Err(e) = std::fs::write(&path, buffer.join("\n")) {
                self.all_log
                    .push_system(format!("[warn] ログ保存失敗: {e}"));
            } else {
                self.all_log
                    .push_system(format!("[system] ログを保存しました: {}", path.display()));
            }
        }
    }

    fn ui_settings_section(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Settings")
            .default_open(false)
            .show(ui, |ui| {
                let prev = (
                    self.config.start_on_boot,
                    self.config.start_minimized_to_tray,
                    self.config.auto_start_watching_on_launch,
                    self.config.auto_restart_on_failure,
                    self.config.max_restart_attempts,
                    self.config.restart_backoff_seconds,
                );
                ui.checkbox(&mut self.config.start_on_boot, "Windows 起動時に自動起動");
                ui.checkbox(
                    &mut self.config.start_minimized_to_tray,
                    "起動時はトレイに最小化",
                );
                ui.checkbox(
                    &mut self.config.auto_start_watching_on_launch,
                    "起動時に有効フォルダの監視を開始",
                );
                ui.checkbox(
                    &mut self.config.auto_restart_on_failure,
                    "watcher 失敗時に自動再起動",
                );
                ui.horizontal(|ui| {
                    ui.label("最大再試行回数:");
                    ui.add(egui::DragValue::new(&mut self.config.max_restart_attempts).range(1..=100));
                    ui.label("バックオフ (秒):");
                    ui.add(
                        egui::DragValue::new(&mut self.config.restart_backoff_seconds).range(1..=3600),
                    );
                });

                let now = (
                    self.config.start_on_boot,
                    self.config.start_minimized_to_tray,
                    self.config.auto_start_watching_on_launch,
                    self.config.auto_restart_on_failure,
                    self.config.max_restart_attempts,
                    self.config.restart_backoff_seconds,
                );
                if prev != now {
                    self.pending_save = true;
                    if prev.0 != now.0 {
                        if let Err(e) = autostart::sync(self.config.start_on_boot) {
                            self.all_log
                                .push_system(format!("[warn] 自動起動設定失敗: {e}"));
                        }
                    }
                }
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_log_events(ctx);
        self.handle_tray_events(ctx);

        if ctx.input(|i| i.viewport().close_requested()) && !self.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.minimized_to_tray = true;
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.ui_server_section(ui);
                ui.add_space(8.0);
                self.ui_folders_section(ui);
                ui.add_space(8.0);
                self.ui_log_section(ui);
                ui.add_space(8.0);
                self.ui_settings_section(ui);
            });
        });

        self.save_if_dirty();
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let ids: Vec<String> = self.watchers.keys().cloned().collect();
        for id in ids {
            if let Some(mut h) = self.watchers.remove(&id) {
                h.request_stop();
                let _ = self.rt.block_on(async { h.stop().await });
            }
        }
    }
}

fn pick_folder_dialog() -> Option<PathBuf> {
    rfd_pick_folder()
}

fn save_file_dialog(default_name: &str) -> Option<PathBuf> {
    rfd_save_file(default_name)
}

fn rfd_pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_folder()
}

fn rfd_save_file(default_name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_file_name(default_name)
        .add_filter("Text", &["txt", "log"])
        .save_file()
}

fn chrono_like_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.to_string()
}
