use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// フレンド情報
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FriendEntry {
    pub name: String,
    /// EndpointId の hex エンコード文字列
    pub endpoint_id: String,
}

/// アプリケーション設定（TOML 永続化）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub name: String,
    #[serde(default)]
    pub friends: Vec<FriendEntry>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            name: "Player".to_string(),
            friends: Vec::new(),
        }
    }
}

impl AppConfig {
    /// 設定ディレクトリ（~/.config/playroom/）
    pub fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("playroom")
    }

    /// 設定ファイルパス（~/.config/playroom/config.toml）
    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    /// 設定ファイルを読み込む。存在しなければデフォルトを返す。
    pub fn load_or_default() -> Self {
        let path = Self::config_path();
        match fs::read_to_string(&path) {
            Ok(contents) => toml::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// 設定ファイルに書き込む。ディレクトリがなければ作成する。
    pub fn save(&self) -> anyhow::Result<()> {
        let dir = Self::config_dir();
        fs::create_dir_all(&dir)?;
        let contents = toml::to_string_pretty(self)?;
        fs::write(Self::config_path(), contents)?;
        Ok(())
    }

    /// フレンドを追加する。同名のフレンドが既にいれば更新する。
    pub fn add_friend(&mut self, name: String, endpoint_id: String) {
        if let Some(existing) = self.friends.iter_mut().find(|f| f.name == name) {
            existing.endpoint_id = endpoint_id;
        } else {
            self.friends.push(FriendEntry { name, endpoint_id });
        }
    }

    /// フレンドを名前で削除する。
    pub fn remove_friend(&mut self, name: &str) {
        self.friends.retain(|f| f.name != name);
    }

    /// フレンドを名前で検索する。
    pub fn find_friend(&self, name: &str) -> Option<&FriendEntry> {
        self.friends.iter().find(|f| f.name == name)
    }
}
