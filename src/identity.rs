//! ユーザー恒久アイデンティティ
//!
//! iroh の `EndpointId` は「デバイス単位」の識別子だが、本モジュールは
//! ユーザー単位のアイデンティティ（複数デバイスで共有される Ed25519 鍵ペア）
//! を扱う。ユーザー公開鍵 = ユーザーIDとして振る舞い、フレンドはこの公開鍵で
//! 相手を識別する。
//!
//! 秘密鍵は `~/.config/playroom/identity.key` に保存され、デバイスペアリング
//! 時に他デバイスへ転送される（ユーザーには露出しない）。

use std::fs;
use std::path::PathBuf;

use ed25519_dalek::{
    Signature, Signer, SigningKey, Verifier, VerifyingKey, SECRET_KEY_LENGTH,
};
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;

/// ユーザー公開鍵のバイト長（Ed25519）
pub const USER_PUBLIC_KEY_LENGTH: usize = 32;
/// 署名長
pub const SIGNATURE_LENGTH: usize = 64;

/// ユーザー公開鍵（32 バイト）
pub type UserPublicKey = [u8; USER_PUBLIC_KEY_LENGTH];
/// 署名（64 バイト）
pub type SignatureBytes = [u8; SIGNATURE_LENGTH];

/// ユーザー恒久鍵ペア
///
/// デバイス間で共有される。新規生成は `generate()`、ディスクから復元は
/// `load_or_generate()`、ペアリングで受信した秘密鍵を保存するには
/// `from_secret_bytes()` + `save()` を使う。
pub struct UserIdentity {
    signing_key: SigningKey,
}

impl UserIdentity {
    /// 新規にランダムな鍵ペアを生成する（新規ユーザー用）。
    pub fn generate() -> Self {
        let mut bytes = [0u8; SECRET_KEY_LENGTH];
        getrandom::fill(&mut bytes).expect("OS RNG must be available");
        let signing_key = SigningKey::from_bytes(&bytes);
        Self { signing_key }
    }

    /// 秘密鍵バイト列（32 バイト）から復元する。
    pub fn from_secret_bytes(bytes: &[u8; SECRET_KEY_LENGTH]) -> Self {
        let signing_key = SigningKey::from_bytes(bytes);
        Self { signing_key }
    }

    /// ディスクから読み込む。存在しなければ新規生成 + 保存する。
    pub fn load_or_generate() -> anyhow::Result<Self> {
        let path = Self::key_path();
        match fs::read(&path) {
            Ok(bytes) if bytes.len() == SECRET_KEY_LENGTH => {
                let mut arr = [0u8; SECRET_KEY_LENGTH];
                arr.copy_from_slice(&bytes);
                Ok(Self::from_secret_bytes(&arr))
            }
            _ => {
                let id = Self::generate();
                id.save()?;
                Ok(id)
            }
        }
    }

    /// 秘密鍵を `~/.config/playroom/identity.key` に保存する。
    pub fn save(&self) -> anyhow::Result<()> {
        let dir = AppConfig::config_dir();
        fs::create_dir_all(&dir)?;
        let path = Self::key_path();
        fs::write(&path, self.signing_key.to_bytes())?;
        // POSIX ではパーミッションを 600 に絞る
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = fs::metadata(&path)?.permissions();
            perm.set_mode(0o600);
            fs::set_permissions(&path, perm)?;
        }
        Ok(())
    }

    /// 秘密鍵ファイルパス
    pub fn key_path() -> PathBuf {
        AppConfig::config_dir().join("identity.key")
    }

    /// 公開鍵を返す
    pub fn public_key(&self) -> UserPublicKey {
        self.signing_key.verifying_key().to_bytes()
    }

    /// 秘密鍵バイト列を返す（ペアリング転送用、取り扱い注意）
    pub fn secret_bytes(&self) -> [u8; SECRET_KEY_LENGTH] {
        self.signing_key.to_bytes()
    }

    /// メッセージに署名する
    pub fn sign(&self, message: &[u8]) -> SignatureBytes {
        self.signing_key.sign(message).to_bytes()
    }
}

/// 公開鍵で署名を検証する
pub fn verify_signature(
    public_key: &UserPublicKey,
    message: &[u8],
    signature: &SignatureBytes,
) -> bool {
    let Ok(vk) = VerifyingKey::from_bytes(public_key) else {
        return false;
    };
    let sig = Signature::from_bytes(signature);
    vk.verify(message, &sig).is_ok()
}

/// 公開鍵を表示用に短縮する（先頭 4 バイト = 8 文字の hex）
pub fn short_key(public_key: &UserPublicKey) -> String {
    hex::encode(&public_key[..4])
}

/// 表示用の名前: `Alice#a3f8b2c1`
pub fn display_name(name: &str, public_key: &UserPublicKey) -> String {
    format!("{}#{}", name, short_key(public_key))
}

/// シリアライズ可能な鍵ラッパ（TOML/postcard で使用）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct UserPublicKeyHex(pub String);

impl UserPublicKeyHex {
    pub fn from_bytes(bytes: &UserPublicKey) -> Self {
        Self(hex::encode(bytes))
    }

    pub fn to_bytes(&self) -> Option<UserPublicKey> {
        let decoded = hex::decode(&self.0).ok()?;
        if decoded.len() != USER_PUBLIC_KEY_LENGTH {
            return None;
        }
        let mut arr = [0u8; USER_PUBLIC_KEY_LENGTH];
        arr.copy_from_slice(&decoded);
        Some(arr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify_roundtrip() {
        let id = UserIdentity::generate();
        let msg = b"hello world";
        let sig = id.sign(msg);
        let pk = id.public_key();
        assert!(verify_signature(&pk, msg, &sig));

        // 改竄されたメッセージは検証失敗
        assert!(!verify_signature(&pk, b"goodbye world", &sig));
    }

    #[test]
    fn secret_bytes_roundtrip() {
        let id1 = UserIdentity::generate();
        let bytes = id1.secret_bytes();
        let id2 = UserIdentity::from_secret_bytes(&bytes);
        assert_eq!(id1.public_key(), id2.public_key());
    }

    #[test]
    fn display_name_format() {
        let pk = [0xa3, 0xf8, 0xb2, 0xc1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(display_name("Alice", &pk), "Alice#a3f8b2c1");
    }

    #[test]
    fn hex_wrapper_roundtrip() {
        let id = UserIdentity::generate();
        let pk = id.public_key();
        let hex = UserPublicKeyHex::from_bytes(&pk);
        assert_eq!(hex.to_bytes(), Some(pk));
    }
}
