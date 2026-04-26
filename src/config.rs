use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AlbumMode {
    None,
    FromFolder,
    Named,
}

impl Default for AlbumMode {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct FolderConfig {
    pub id: String,
    pub path: PathBuf,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub recursive: bool,
    #[serde(default)]
    pub album_mode: AlbumMode,
    #[serde(default)]
    pub album_name: String,
    #[serde(default)]
    pub include_hidden: bool,
    #[serde(default)]
    pub ignore_patterns: Vec<String>,
    #[serde(default = "default_concurrency")]
    pub concurrency: u32,
}

fn default_true() -> bool {
    true
}
fn default_concurrency() -> u32 {
    4
}

impl FolderConfig {
    pub fn new(path: PathBuf) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            path,
            enabled: true,
            recursive: true,
            album_mode: AlbumMode::None,
            album_name: String::new(),
            include_hidden: false,
            ignore_patterns: Vec::new(),
            concurrency: 4,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub server_url: String,
    #[serde(default)]
    pub start_on_boot: bool,
    #[serde(default = "default_true")]
    pub start_minimized_to_tray: bool,
    #[serde(default = "default_true")]
    pub auto_start_watching_on_launch: bool,
    #[serde(default = "default_true")]
    pub auto_restart_on_failure: bool,
    #[serde(default = "default_max_restart")]
    pub max_restart_attempts: u32,
    #[serde(default = "default_backoff")]
    pub restart_backoff_seconds: u64,
    #[serde(default)]
    pub folders: Vec<FolderConfig>,
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}
fn default_max_restart() -> u32 {
    5
}
fn default_backoff() -> u64 {
    30
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            server_url: String::new(),
            start_on_boot: false,
            start_minimized_to_tray: true,
            auto_start_watching_on_launch: true,
            auto_restart_on_failure: true,
            max_restart_attempts: 5,
            restart_backoff_seconds: 30,
            folders: Vec::new(),
        }
    }
}

pub struct LoadResult {
    pub config: Config,
    pub warning: Option<String>,
}

impl Config {
    pub fn load_or_default() -> LoadResult {
        let Some(path) = config_path() else {
            return LoadResult {
                config: Self::default(),
                warning: Some("config ディレクトリを取得できません。デフォルトで起動します".into()),
            };
        };
        if !path.exists() {
            return LoadResult {
                config: Self::default(),
                warning: None,
            };
        }
        match std::fs::read_to_string(&path) {
            Ok(s) => match serde_json::from_str::<Config>(&s) {
                Ok(cfg) => {
                    if cfg.schema_version > SCHEMA_VERSION {
                        LoadResult {
                            config: Self::default(),
                            warning: Some(format!(
                                "未知の schema_version={} のため、デフォルト設定で起動しました (元ファイルは保持)",
                                cfg.schema_version
                            )),
                        }
                    } else {
                        LoadResult {
                            config: cfg,
                            warning: None,
                        }
                    }
                }
                Err(e) => LoadResult {
                    config: Self::default(),
                    warning: Some(format!("config 解析失敗: {e}. デフォルトで起動")),
                },
            },
            Err(e) => LoadResult {
                config: Self::default(),
                warning: Some(format!("config 読み込み失敗: {e}. デフォルトで起動")),
            },
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path().context("config パスを取得できません")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("config ディレクトリ作成失敗")?;
        }
        let s = serde_json::to_string_pretty(self).context("config シリアライズ失敗")?;
        std::fs::write(&path, s).context("config 書き込み失敗")?;
        Ok(())
    }
}

pub fn config_path() -> Option<PathBuf> {
    use directories::ProjectDirs;
    let dirs = ProjectDirs::from("", "", "immich-auto-uploader")?;
    Some(dirs.config_dir().join("config.json"))
}
