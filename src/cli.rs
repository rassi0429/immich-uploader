use anyhow::{Result, anyhow};
use std::path::PathBuf;
use tokio::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const INSTALL_HINT: &str = "\n\nインストール手順:\n  1. Node.js をインストール (https://nodejs.org/)\n  2. ターミナルで `npm install -g @immich/cli` を実行\n  3. アプリを再起動";

pub fn resolve_immich_path() -> Result<PathBuf> {
    which::which("immich")
        .map_err(|e| anyhow!("Immich CLI (`immich`) が PATH に見つかりません: {e}{INSTALL_HINT}"))
}

pub fn build_command() -> Result<Command> {
    let path = resolve_immich_path()?;
    #[allow(unused_mut)]
    let mut cmd = Command::new(path);
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(cmd)
}

pub async fn check_immich_installed() -> Result<String> {
    let mut cmd = build_command()?;
    let output = cmd
        .arg("--version")
        .output()
        .await
        .map_err(|e| anyhow!("Immich CLI の起動に失敗しました: {e}{INSTALL_HINT}"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "immich --version が失敗しました: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub async fn login(url: &str, key: &str) -> Result<String> {
    if url.trim().is_empty() {
        return Err(anyhow!("サーバー URL が空です"));
    }
    if key.trim().is_empty() {
        return Err(anyhow!("API キーが空です"));
    }
    let mut cmd = build_command()?;
    let output = cmd
        .args(["login", url, key])
        .output()
        .await
        .map_err(|e| anyhow!("immich login の起動に失敗しました: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(anyhow!(
            "ログイン失敗:\n{}\n{}",
            stdout.trim(),
            stderr.trim()
        ));
    }
    let combined = format!("{}\n{}", stdout.trim(), stderr.trim());
    Ok(combined.trim().to_string())
}
