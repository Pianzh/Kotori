//! 凭据存储本身的那几个 RPC：解锁 / 锁定 / 设主密码 / 删掉主密码文件。
//!
//! 与 `actions.rs` 分开：那边是"把存档搬来搬去"，这边是"凭据放在哪、能不能打开"，
//! 两者的失败方式完全不同（一个是网络，一个是密码）。

use serde_json::{Value, json};

use super::{Daemon, Password};
use crate::secrets::{EncryptedFile, Keyring, SecretKey, plain::PlainFile};

impl Daemon {
    /// Unlock the master-password file with the password the user just typed.
    pub(in crate::daemon) fn rpc_sync_unlock(&self, password: Password) -> Result<Value, String> {
        let keyring = self.sync.keyring();
        let Some(file) = keyring.encrypted_store() else {
            // 如实报现在用的是哪一级:说"存在系统密钥环"在只跑内存的机器上是假话。
            return Err(format!(
                "当前不需要解锁（凭据存在{}里）",
                keyring.describe()
            ));
        };
        file.unlock(&password.password).map_err(|e| e.to_string())?;
        tracing::info!("凭据文件已解锁");
        Ok(json!({ "unlocked": true, "store": keyring.kind() }))
    }

    /// Move whatever credentials we have into a master-password file.
    ///
    /// This is the escape hatch for machines with no OS keyring: without it,
    /// every restart would ask for the B2 keys again, which is exactly what
    /// makes unattended sync impossible on those systems.
    ///
    /// ⚠ 两条"绝不"（B1 / B2）：
    ///   * **读不出来的凭据绝不当成"没存过"** —— 锁着的时候重设主密码，从前会把
    ///     `Err(Locked)` 和 `Ok(None)` 一起 `filter_map` 掉，于是 `create`（覆盖写）
    ///     把用户的凭据**换成一个空文件**，界面还报"已加密保存"；
    ///   * **明文文件搬完之后必须删掉**，而且要**先回读确认**每一条都进了加密文件。
    pub(in crate::daemon) fn rpc_sync_set_master_password(
        &self,
        password: Password,
    ) -> Result<Value, String> {
        let path = self.sync.secrets_path();
        let existing = EncryptedFile::new(&path);
        if existing.exists() && !password.force {
            return Err(
                "已经有一个主密码凭据文件了；重设主密码会重新加密它（旧密码立即失效），确认请再点一次"
                    .to_string(),
            );
        }

        let current = self.sync.keyring();
        let mut entries: Vec<(SecretKey, String)> = Vec::new();
        for key in SecretKey::ALL {
            match current.get(key) {
                Ok(Some(value)) => entries.push((key, value)),
                // 没存过是正常状态（比如只设了 kopia 密码）。
                Ok(None) => {}
                // 读不出来 = **不知道里面有什么**，绝不能当成"没有"。
                Err(error) => {
                    return Err(format!(
                        "有凭据现在读不出来（{error}）—— 先用当前主密码解锁再重设；\
                         这一次一个字节都没写"
                    ));
                }
            }
        }

        existing
            .create(&password.password, &entries)
            .map_err(|e| e.to_string())?;
        // Adopt the very handle we just sealed: a fresh one would be locked.
        self.sync.adopt(Keyring::from_encrypted(existing));

        // B2：明文文件里的东西已经进了加密文件 ⇒ 明文不该继续留在盘上。
        let plain_warning = self.drop_migrated_plaintext().err();

        tracing::info!(
            "凭据已存入主密码文件 {}（{} 条）",
            path.display(),
            entries.len()
        );
        Ok(json!({
            "stored": true,
            "path": path.display().to_string(),
            "count": entries.len(),
            "plain_warning": plain_warning,
        }))
    }

    /// 明文凭据文件在成功搬进主密码文件之后必须消失（B2）。
    ///
    /// 三道关，缺一不可：
    ///   ① 明文文件本来就不在 ⇒ 什么都不用做；
    ///   ② **明文里每一条**都必须能在刚写好的加密文件里读回来、值也一样 —— 对不上就
    ///      **不删**：宁可多留一份明文，也不能把用户唯一的一份凭据删掉；
    ///   ③ 删失败 ⇒ 返回一句警告，由调用方说给用户听（**绝不假装成功**）。
    fn drop_migrated_plaintext(&self) -> Result<(), String> {
        let path = self.sync.plain_path();
        let plain = PlainFile::new(&path);
        if !plain.exists() {
            return Ok(());
        }
        let inside = plain
            .load()
            .map_err(|e| format!("明文凭据文件读不出来（{e}），先留着没删"))?;
        let keyring = self.sync.keyring();
        for (key, value) in &inside {
            match keyring.get(*key) {
                Ok(Some(back)) if back == *value => {}
                other => {
                    return Err(format!(
                        "明文凭据文件没有删掉：{} 没能确认已经写进加密文件（{other:?}）",
                        key.account()
                    ));
                }
            }
        }
        plain
            .remove()
            .map_err(|e| format!("明文凭据文件没有删掉：{e}"))?;
        tracing::info!("明文凭据已搬进主密码文件，原文件已删除：{}", path.display());
        Ok(())
    }

    /// Delete the master-password file.
    ///
    /// The credentials in it go with it; on a machine with no keyring that
    /// means they are gone. The UI asks twice.
    pub(in crate::daemon) fn rpc_sync_clear_master_password(&self) -> Result<Value, String> {
        let path = self.sync.secrets_path();
        let file = EncryptedFile::new(&path);
        if !file.exists() {
            return Err("没有主密码凭据文件".to_string());
        }
        file.remove().map_err(|e| e.to_string())?;

        // 重挑一次存储:删掉主密码文件之后该轮到下一级了 —— 便携目录里是明文
        // 文件,别的地方是密钥环(在跑的话)/明文,见 `Keyring::open_at`。
        let plain = self.sync.plain_path();
        let fresh = Keyring::open_at(&path, &plain, crate::config::is_portable_config());
        self.sync.adopt(fresh);
        tracing::warn!("主密码凭据文件已删除: {}", path.display());
        Ok(json!({ "removed": true }))
    }

    /// Forget the key until the password is entered again.
    pub(in crate::daemon) fn rpc_sync_lock(&self) -> Result<Value, String> {
        let keyring = self.sync.keyring();
        let Some(file) = keyring.encrypted_store() else {
            return Err("当前不是主密码凭据文件模式".to_string());
        };
        file.lock();
        Ok(json!({ "locked": true, "store": keyring.kind() }))
    }
}
