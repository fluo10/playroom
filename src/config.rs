//! アプリケーション設定の永続化
//!
//! 主に3つのデータを管理する：
//!   - `name`: 自分の表示名（playroom 共通）
//!   - `my_devices`: 自分の所有デバイス一覧（per-device LWW で複数デバイス間同期）
//!   - `friends`: フレンド一覧（per-friend LWW + フレンド側署名付き自己情報のキャッシュ）
//!
//! ユーザー秘密鍵は `identity.rs` が別ファイル（`identity.key`）に保存する。

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::identity::UserPublicKeyHex;
use crate::protocol::FriendSelfInfo;

/// 現在時刻を unix ms で返す。同期用タイムスタンプとして使用。
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 自分が所有するデバイスのエントリ（per-device LWW）。
///
/// 書き手は自分の複数デバイス。キーは `endpoint_id`。
/// last_modified が新しい方が勝つ（削除は tombstone で表現）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MyDeviceEntry {
    /// デバイス識別子（iroh EndpointId の hex）
    pub endpoint_id: String,
    /// 人間向けラベル（"Laptop", "Phone" 等）
    pub label: String,
    /// ペアリング/登録時刻（unix ms）
    pub added_at: u64,
    /// 最終更新時刻（unix ms） — LWW のキー
    pub last_modified: u64,
    /// 削除マーカー
    #[serde(default)]
    pub tombstone: bool,
}

/// フレンドエントリ。
///
/// - `user_public_key` / `petname` / `tombstone` / `added_at`: 自分が書き手（per-friend LWW）
/// - `cached_self_info`: フレンド側が書き手（署名付き、version で単調増加）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FriendEntry {
    /// キー（不変、hex）
    pub user_public_key: UserPublicKeyHex,
    /// 自分がつけたあだ名（任意）
    #[serde(default)]
    pub petname: Option<String>,
    /// フレンドから取得した署名付き自己情報（検証済みのみ保存）
    #[serde(default)]
    pub cached_self_info: Option<FriendSelfInfo>,
    /// 追加時刻（unix ms）
    pub added_at: u64,
    /// 最終更新時刻（unix ms） — LWW のキー
    pub last_modified: u64,
    /// 削除マーカー
    #[serde(default)]
    pub tombstone: bool,
}

impl FriendEntry {
    /// 表示名を取得する。petname があれば優先、なければ cached_self_info の name、
    /// いずれも無ければ公開鍵の先頭を使う。
    pub fn display_name(&self) -> String {
        if let Some(p) = &self.petname {
            return p.clone();
        }
        if let Some(info) = &self.cached_self_info {
            return info.name.clone();
        }
        format!("Unknown#{}", &self.user_public_key.0[..8.min(self.user_public_key.0.len())])
    }
}

/// アプリケーション設定（TOML 永続化）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// 自分の表示名
    pub name: String,
    /// 自分が所有するデバイス一覧
    #[serde(default)]
    pub my_devices: Vec<MyDeviceEntry>,
    /// フレンド一覧
    #[serde(default)]
    pub friends: Vec<FriendEntry>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            name: "Player".to_string(),
            my_devices: Vec::new(),
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

    pub fn load_or_default() -> Self {
        let path = Self::config_path();
        match fs::read_to_string(&path) {
            Ok(contents) => toml::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let dir = Self::config_dir();
        fs::create_dir_all(&dir)?;
        let contents = toml::to_string_pretty(self)?;
        fs::write(Self::config_path(), contents)?;
        Ok(())
    }

    // ── フレンド操作 ──

    /// フレンドを追加する（既存なら更新）。同時に last_modified を現在時刻で更新。
    pub fn upsert_friend(&mut self, user_public_key: UserPublicKeyHex, petname: Option<String>) {
        let now = now_unix_ms();
        if let Some(existing) = self
            .friends
            .iter_mut()
            .find(|f| f.user_public_key == user_public_key)
        {
            existing.petname = petname;
            existing.tombstone = false;
            existing.last_modified = now;
        } else {
            self.friends.push(FriendEntry {
                user_public_key,
                petname,
                cached_self_info: None,
                added_at: now,
                last_modified: now,
                tombstone: false,
            });
        }
    }

    /// フレンドを論理削除する。
    pub fn remove_friend(&mut self, user_public_key: &UserPublicKeyHex) {
        if let Some(existing) = self
            .friends
            .iter_mut()
            .find(|f| f.user_public_key == *user_public_key)
        {
            existing.tombstone = true;
            existing.last_modified = now_unix_ms();
        }
    }

    /// フレンドを検索する（tombstone 済みは除外）。
    pub fn find_friend(&self, user_public_key: &UserPublicKeyHex) -> Option<&FriendEntry> {
        self.friends
            .iter()
            .find(|f| f.user_public_key == *user_public_key && !f.tombstone)
    }

    /// フレンドから受け取った署名済み FriendSelfInfo を保存する。
    /// 署名検証と version チェックを行い、新しければ更新する。
    pub fn update_friend_self_info(
        &mut self,
        info: FriendSelfInfo,
    ) -> Result<bool, UpdateSelfInfoError> {
        if !info.verify() {
            return Err(UpdateSelfInfoError::InvalidSignature);
        }
        let key = UserPublicKeyHex::from_bytes(&info.user_public_key);
        let Some(entry) = self.friends.iter_mut().find(|f| f.user_public_key == key) else {
            return Err(UpdateSelfInfoError::UnknownFriend);
        };
        // 既存より古いバージョンは無視
        if let Some(current) = &entry.cached_self_info {
            if info.version <= current.version {
                return Ok(false);
            }
        }
        entry.cached_self_info = Some(info);
        Ok(true)
    }

    // ── 自デバイス操作 ──

    pub fn upsert_my_device(&mut self, endpoint_id: String, label: String) {
        let now = now_unix_ms();
        if let Some(existing) = self
            .my_devices
            .iter_mut()
            .find(|d| d.endpoint_id == endpoint_id)
        {
            existing.label = label;
            existing.tombstone = false;
            existing.last_modified = now;
        } else {
            self.my_devices.push(MyDeviceEntry {
                endpoint_id,
                label,
                added_at: now,
                last_modified: now,
                tombstone: false,
            });
        }
    }

    pub fn remove_my_device(&mut self, endpoint_id: &str) {
        if let Some(existing) = self
            .my_devices
            .iter_mut()
            .find(|d| d.endpoint_id == endpoint_id)
        {
            existing.tombstone = true;
            existing.last_modified = now_unix_ms();
        }
    }

    // ── 同期: LWW マージ ──

    /// 別デバイスから受け取った my_devices / friends リストを自分の状態にマージする。
    /// 戻り値は何かしら更新があったかどうか。
    pub fn merge_from_peer(&mut self, peer: &AppConfig) -> bool {
        let mut changed = false;
        changed |= merge_my_devices(&mut self.my_devices, &peer.my_devices);
        changed |= merge_friends(&mut self.friends, &peer.friends);
        changed
    }
}

/// my_devices の per-device LWW マージ。
fn merge_my_devices(local: &mut Vec<MyDeviceEntry>, remote: &[MyDeviceEntry]) -> bool {
    let mut changed = false;
    for r in remote {
        match local.iter_mut().find(|l| l.endpoint_id == r.endpoint_id) {
            Some(l) if r.last_modified > l.last_modified => {
                *l = r.clone();
                changed = true;
            }
            None => {
                local.push(r.clone());
                changed = true;
            }
            _ => {}
        }
    }
    changed
}

/// friends の per-friend LWW マージ。cached_self_info はフレンド署名 version で独立に比較。
fn merge_friends(local: &mut Vec<FriendEntry>, remote: &[FriendEntry]) -> bool {
    let mut changed = false;
    for r in remote {
        match local.iter_mut().find(|l| l.user_public_key == r.user_public_key) {
            Some(l) => {
                // メタデータ（petname, tombstone）は LWW
                if r.last_modified > l.last_modified {
                    l.petname = r.petname.clone();
                    l.tombstone = r.tombstone;
                    l.added_at = l.added_at.min(r.added_at); // 追加時刻は古い方を保持
                    l.last_modified = r.last_modified;
                    changed = true;
                }
                // cached_self_info は署名検証 + version 比較
                if let Some(r_info) = &r.cached_self_info {
                    if r_info.verify()
                        && r_info.user_public_key == r.user_public_key.to_bytes().unwrap_or_default()
                    {
                        let should_update = match &l.cached_self_info {
                            Some(l_info) => r_info.version > l_info.version,
                            None => true,
                        };
                        if should_update {
                            l.cached_self_info = Some(r_info.clone());
                            changed = true;
                        }
                    }
                }
            }
            None => {
                let mut entry = r.clone();
                // 不正な署名は破棄して取り込む
                if let Some(info) = &entry.cached_self_info {
                    if !info.verify()
                        || info.user_public_key != r.user_public_key.to_bytes().unwrap_or_default()
                    {
                        entry.cached_self_info = None;
                    }
                }
                local.push(entry);
                changed = true;
            }
        }
    }
    changed
}

/// `FriendSelfInfo` 更新時のエラー
#[derive(Debug, thiserror::Error)]
pub enum UpdateSelfInfoError {
    #[error("signature verification failed")]
    InvalidSignature,
    #[error("unknown friend: not in friend list")]
    UnknownFriend,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::UserIdentity;
    use crate::protocol::FriendSelfInfo;

    fn make_friend(key_hex: &str, petname: Option<&str>, last_modified: u64) -> FriendEntry {
        FriendEntry {
            user_public_key: UserPublicKeyHex(key_hex.to_string()),
            petname: petname.map(String::from),
            cached_self_info: None,
            added_at: last_modified,
            last_modified,
            tombstone: false,
        }
    }

    #[test]
    fn merge_my_devices_picks_newer() {
        let mut local = vec![MyDeviceEntry {
            endpoint_id: "dev1".into(),
            label: "Old".into(),
            added_at: 1,
            last_modified: 10,
            tombstone: false,
        }];
        let remote = vec![MyDeviceEntry {
            endpoint_id: "dev1".into(),
            label: "New".into(),
            added_at: 1,
            last_modified: 20,
            tombstone: false,
        }];
        assert!(merge_my_devices(&mut local, &remote));
        assert_eq!(local[0].label, "New");
    }

    #[test]
    fn merge_my_devices_keeps_newer_local() {
        let mut local = vec![MyDeviceEntry {
            endpoint_id: "dev1".into(),
            label: "Local".into(),
            added_at: 1,
            last_modified: 100,
            tombstone: false,
        }];
        let remote = vec![MyDeviceEntry {
            endpoint_id: "dev1".into(),
            label: "Remote".into(),
            added_at: 1,
            last_modified: 50,
            tombstone: false,
        }];
        assert!(!merge_my_devices(&mut local, &remote));
        assert_eq!(local[0].label, "Local");
    }

    #[test]
    fn merge_my_devices_adds_new() {
        let mut local: Vec<MyDeviceEntry> = vec![];
        let remote = vec![MyDeviceEntry {
            endpoint_id: "dev2".into(),
            label: "New".into(),
            added_at: 1,
            last_modified: 1,
            tombstone: false,
        }];
        assert!(merge_my_devices(&mut local, &remote));
        assert_eq!(local.len(), 1);
    }

    #[test]
    fn merge_friends_concurrent_additions_both_survive() {
        // A と B がオフラインで別々にフレンドを追加したシナリオ
        let mut a = vec![make_friend("aa", Some("Alice"), 10)];
        let b = vec![make_friend("bb", Some("Bob"), 10)];
        assert!(merge_friends(&mut a, &b));
        assert_eq!(a.len(), 2);
    }

    #[test]
    fn merge_friends_tombstone_wins_newer() {
        let mut local = vec![make_friend("aa", Some("Alice"), 10)];
        let mut remote = vec![make_friend("aa", Some("Alice"), 20)];
        remote[0].tombstone = true;
        assert!(merge_friends(&mut local, &remote));
        assert!(local[0].tombstone);
    }

    #[test]
    fn merge_friends_cached_self_info_uses_version() {
        let id = UserIdentity::generate();
        let pk_hex = UserPublicKeyHex::from_bytes(&id.public_key());

        let old = FriendSelfInfo::sign_new(&id, "Alice".into(), vec![], 1);
        let new = FriendSelfInfo::sign_new(&id, "Alice_v2".into(), vec![], 2);

        let mut local_entry = FriendEntry {
            user_public_key: pk_hex.clone(),
            petname: None,
            cached_self_info: Some(old),
            added_at: 1,
            last_modified: 1,
            tombstone: false,
        };
        local_entry.user_public_key = pk_hex.clone();

        let remote_entry = FriendEntry {
            user_public_key: pk_hex,
            petname: None,
            cached_self_info: Some(new),
            added_at: 1,
            last_modified: 1, // last_modified 変化なし
            tombstone: false,
        };

        let mut local = vec![local_entry];
        assert!(merge_friends(&mut local, &[remote_entry]));
        assert_eq!(local[0].cached_self_info.as_ref().unwrap().version, 2);
    }

    #[test]
    fn merge_friends_tampered_self_info_dropped_on_new_insert() {
        let id = UserIdentity::generate();
        let pk_hex = UserPublicKeyHex::from_bytes(&id.public_key());
        let mut tampered = FriendSelfInfo::sign_new(&id, "Alice".into(), vec![], 1);
        tampered.name = "Mallory".into(); // 署名が無効になる

        let remote_entry = FriendEntry {
            user_public_key: pk_hex,
            petname: None,
            cached_self_info: Some(tampered),
            added_at: 1,
            last_modified: 1,
            tombstone: false,
        };

        let mut local: Vec<FriendEntry> = vec![];
        merge_friends(&mut local, &[remote_entry]);
        assert_eq!(local.len(), 1);
        assert!(local[0].cached_self_info.is_none());
    }
}
