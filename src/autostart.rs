use anyhow::{Context, Result};
use winreg::RegKey;
use winreg::enums::*;

const APP_VALUE_NAME: &str = "ImmichAutoUploader";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

pub fn enable() -> Result<()> {
    let exe = std::env::current_exe().context("実行ファイルパスの取得に失敗しました")?;
    let value = format!("\"{}\"", exe.display());
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(RUN_KEY)
        .context("Run レジストリキーの作成に失敗しました")?;
    key.set_value(APP_VALUE_NAME, &value)
        .context("Run レジストリ値の設定に失敗しました")?;
    Ok(())
}

pub fn disable() -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE) {
        let _ = key.delete_value(APP_VALUE_NAME);
    }
    Ok(())
}

pub fn sync(enabled: bool) -> Result<()> {
    if enabled {
        enable()
    } else {
        disable()
    }
}
