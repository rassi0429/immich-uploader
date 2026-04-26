use crate::ansi::LineProcessor;
use crate::config::{AlbumMode, FolderConfig};
use anyhow::Result;
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
pub enum LogEvent {
    Line {
        folder_id: String,
        line: String,
        partial: bool,
    },
    Started {
        folder_id: String,
        pid: u32,
    },
    Exited {
        folder_id: String,
        code: Option<i32>,
        was_canceled: bool,
    },
    Error {
        folder_id: String,
        message: String,
    },
}

pub struct WatcherHandle {
    cancel: Option<oneshot::Sender<()>>,
    join: Option<JoinHandle<()>>,
}

impl WatcherHandle {
    pub async fn stop(mut self) {
        if let Some(tx) = self.cancel.take() {
            let _ = tx.send(());
        }
        if let Some(j) = self.join.take() {
            let _ = j.await;
        }
    }

    pub fn request_stop(&mut self) {
        if let Some(tx) = self.cancel.take() {
            let _ = tx.send(());
        }
    }
}

pub fn build_upload_args(folder: &FolderConfig, watch: bool) -> Vec<String> {
    let mut args: Vec<String> = vec!["upload".into()];
    if watch {
        args.push("--watch".into());
    }
    if folder.recursive {
        args.push("--recursive".into());
    }
    if folder.include_hidden {
        args.push("--include-hidden".into());
    }
    args.push("--concurrency".into());
    args.push(folder.concurrency.to_string());
    match folder.album_mode {
        AlbumMode::None => {}
        AlbumMode::FromFolder => args.push("--album".into()),
        AlbumMode::Named => {
            if !folder.album_name.trim().is_empty() {
                args.push("--album-name".into());
                args.push(folder.album_name.clone());
            }
        }
    }
    for pat in &folder.ignore_patterns {
        if !pat.trim().is_empty() {
            args.push("--ignore".into());
            args.push(pat.clone());
        }
    }
    args.push(folder.path.to_string_lossy().to_string());
    args
}

fn make_command(args: &[String]) -> Result<Command> {
    let mut cmd = crate::cli::build_command()?;
    cmd.args(args)
        .env("FORCE_COLOR", "0")
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    Ok(cmd)
}

pub fn spawn_watcher(
    folder: FolderConfig,
    log_tx: UnboundedSender<LogEvent>,
    auto_restart: bool,
    max_attempts: u32,
    backoff_secs: u64,
) -> WatcherHandle {
    let folder_id = folder.id.clone();
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let folder_id_for_task = folder_id.clone();

    let join = tokio::spawn(async move {
        run_watcher_loop(
            folder,
            folder_id_for_task,
            log_tx,
            auto_restart,
            max_attempts,
            backoff_secs,
            cancel_rx,
        )
        .await;
    });

    let _ = folder_id;
    WatcherHandle {
        cancel: Some(cancel_tx),
        join: Some(join),
    }
}

async fn run_watcher_loop(
    folder: FolderConfig,
    folder_id: String,
    log_tx: UnboundedSender<LogEvent>,
    auto_restart: bool,
    max_attempts: u32,
    backoff_secs: u64,
    mut cancel_rx: oneshot::Receiver<()>,
) {
    let mut attempts: u32 = 0;
    loop {
        let args = build_upload_args(&folder, true);
        let mut cmd = match make_command(&args) {
            Ok(c) => c,
            Err(e) => {
                let _ = log_tx.send(LogEvent::Error {
                    folder_id: folder_id.clone(),
                    message: format!("immich コマンド解決失敗: {e}"),
                });
                return;
            }
        };
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = log_tx.send(LogEvent::Error {
                    folder_id: folder_id.clone(),
                    message: format!("immich プロセス起動失敗: {e}"),
                });
                return;
            }
        };
        let pid = child.id().unwrap_or(0);
        let _ = log_tx.send(LogEvent::Started {
            folder_id: folder_id.clone(),
            pid,
        });

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let mut stream_handles = Vec::new();
        if let Some(s) = stdout {
            stream_handles.push(tokio::spawn(forward_stream(
                s,
                folder_id.clone(),
                log_tx.clone(),
            )));
        }
        if let Some(s) = stderr {
            stream_handles.push(tokio::spawn(forward_stream(
                s,
                folder_id.clone(),
                log_tx.clone(),
            )));
        }

        let mut canceled = false;
        let exit_code: Option<i32> = tokio::select! {
            res = child.wait() => {
                match res {
                    Ok(status) => status.code(),
                    Err(e) => {
                        let _ = log_tx.send(LogEvent::Error {
                            folder_id: folder_id.clone(),
                            message: format!("子プロセス wait 失敗: {e}"),
                        });
                        None
                    }
                }
            }
            _ = &mut cancel_rx => {
                canceled = true;
                if pid != 0 {
                    let _ = kill_tree::tokio::kill_tree(pid).await;
                } else {
                    let _ = child.kill().await;
                }
                let _ = child.wait().await;
                None
            }
        };

        for h in stream_handles {
            let _ = h.await;
        }

        let _ = log_tx.send(LogEvent::Exited {
            folder_id: folder_id.clone(),
            code: exit_code,
            was_canceled: canceled,
        });

        if canceled {
            return;
        }
        if !auto_restart {
            return;
        }
        attempts += 1;
        if attempts >= max_attempts {
            let _ = log_tx.send(LogEvent::Error {
                folder_id: folder_id.clone(),
                message: format!("最大再試行回数 {max_attempts} に達しました。停止します"),
            });
            return;
        }
        let _ = log_tx.send(LogEvent::Error {
            folder_id: folder_id.clone(),
            message: format!(
                "{backoff_secs} 秒後に再起動します (試行 {}/{})",
                attempts, max_attempts
            ),
        });
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)) => {}
            _ = &mut cancel_rx => return,
        }
    }
}

async fn forward_stream<R>(mut reader: R, folder_id: String, log_tx: UnboundedSender<LogEvent>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut processor = LineProcessor::new();
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => {
                let cur = processor.current_line().to_string();
                if !cur.is_empty() {
                    let _ = log_tx.send(LogEvent::Line {
                        folder_id: folder_id.clone(),
                        line: cur,
                        partial: false,
                    });
                }
                break;
            }
            Ok(n) => {
                let lines = processor.process(&buf[..n]);
                for line in lines {
                    let _ = log_tx.send(LogEvent::Line {
                        folder_id: folder_id.clone(),
                        line,
                        partial: false,
                    });
                }
                let cur = processor.current_line().to_string();
                if !cur.is_empty() {
                    let _ = log_tx.send(LogEvent::Line {
                        folder_id: folder_id.clone(),
                        line: cur,
                        partial: true,
                    });
                }
            }
            Err(_) => break,
        }
    }
}

pub async fn run_sync_now(folder: FolderConfig, log_tx: UnboundedSender<LogEvent>) -> Result<()> {
    let args = build_upload_args(&folder, false);
    let mut cmd = make_command(&args)?;
    let mut child = cmd.spawn()?;
    let pid = child.id().unwrap_or(0);
    let _ = log_tx.send(LogEvent::Started {
        folder_id: folder.id.clone(),
        pid,
    });
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let mut handles = Vec::new();
    if let Some(s) = stdout {
        handles.push(tokio::spawn(forward_stream(
            s,
            folder.id.clone(),
            log_tx.clone(),
        )));
    }
    if let Some(s) = stderr {
        handles.push(tokio::spawn(forward_stream(
            s,
            folder.id.clone(),
            log_tx.clone(),
        )));
    }
    let status = child.wait().await?;
    for h in handles {
        let _ = h.await;
    }
    let _ = log_tx.send(LogEvent::Exited {
        folder_id: folder.id.clone(),
        code: status.code(),
        was_canceled: false,
    });
    Ok(())
}
