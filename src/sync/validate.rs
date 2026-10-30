//! 同步设置与凭据的校验：**在动任何数据之前**把不能开始的配置挡下来。
//!
//! 单独成文件，是因为这里只回答"能不能开始"：不构造 rclone 参数
//! （见 `rclone_args`），不拼远端路径（见 `remote_paths`），也不读密钥环里的值，
//! 只判断缺了什么、哪个值一定用不了。

use super::SyncError;
use crate::config::{SyncConfig, SyncEngine};

/// Validate the settings that must be present before anything is attempted.
///
/// The only thing a B2 setup really needs is the bucket name: credentials live
/// in the keyring and rclone discovers the API host itself.
pub fn validate(settings: &SyncConfig) -> Result<(), SyncError> {
    if !settings.enabled {
        return Err(SyncError::NotEnabled);
    }
    if settings.bucket.trim().is_empty() {
        return Err(SyncError::Config("还没有填写 bucket".to_string()));
    }
    validate_endpoint(&settings.endpoint)?;
    Ok(())
}

/// Check the optional endpoint override.
///
/// This exists because the value the B2 console shows most prominently — the
/// `s3.<region>.backblazeb2.com` S3 endpoint — is **not** usable here: the
/// native B2 backend speaks the B2 API, and rclone would POST to
/// `https://s3.<region>.backblazeb2.com/b2api/...`, which does not exist.
/// Failing with that explanation beats a mystery 404 on the user's first sync.
pub fn validate_endpoint(endpoint: &str) -> Result<(), SyncError> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }
    // ⚠ 只看**主机名**：带不带协议头、后面有没有路径，都不该改变这一刀。
    //
    // 从前的写法是 `endpoint.starts_with("s3.")`，于是 B2 控制台直接复制出来的那种
    // （带 `https://`，也正是这段文案自己在要求"写完整"的那种）**恰好穿过去**，只拦住
    // 不带协议头的裸域名 —— 校验本来要拦的那次 404 于是照样发生（BB-01）。
    let host = endpoint
        .split_once("://")
        .map_or(endpoint, |(_scheme, rest)| rest)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if host.starts_with("s3.") && host.ends_with(".backblazeb2.com") {
        return Err(SyncError::Config(format!(
            "{endpoint} 是 B2 的 S3 兼容接口地址，原生 B2 后端用不上——把它留空即可，\n\n             rclone 会自己找到正确的 API 地址。这个字段只在需要指定特定区域端点时才填，\n             而且要写完整（含 https://）"
        )));
    }
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        return Err(SyncError::Config(format!(
            "endpoint 要写完整地址（含 https://），或者直接留空（推荐）：{endpoint}"
        )));
    }
    Ok(())
}

/// 这一次运行需不需要 B2 那一对凭据。
///
/// rclone 那条路一直只走 B2。**kopia 不是**：它可以钉在本地目录仓库上
/// （`KOTORI_KOPIA_REPOSITORY`，NAS / 挂在别处的盘，见 `sync::engine`），那条路
/// 一个 B2 密码都用不上 —— 从前无条件索要，于是这种配置在设置页里被引去填一段
/// 根本用不着的东西，填不进去就是死路（SC-08）。
pub fn needs_b2_credentials(settings: &SyncConfig) -> bool {
    match settings.engine {
        SyncEngine::Rclone => true,
        SyncEngine::Kopia => crate::sync::engine::filesystem_repository().is_none(),
    }
}

/// Check the secrets a run needs. Kept separate from [`validate`] so the
/// structural checks stay pure (and testable without a keyring).
///
/// 这里只有 B2 凭据：同步密码随加密一起没了（rclone 这条路不提供加密）。
/// `needs_b2` 由 [`needs_b2_credentials`] 算出来 —— 显式传进来，是为了让这条
/// 函数保持"问它一个答案"的形状，不必在测试里动环境变量。
pub fn validate_secrets(
    keyring: &crate::secrets::Keyring,
    needs_b2: bool,
) -> Result<(), SyncError> {
    // A locked store is not an empty one. Saying "no credentials yet" here
    // would send the user to re-enter keys that are already on disk.
    if let crate::secrets::StoreKind::EncryptedFile { locked: true, path } = keyring.kind() {
        return Err(SyncError::Config(format!(
            "凭据文件 {path} 已锁定，请先用主密码解锁"
        )));
    }

    if !needs_b2 {
        return Ok(());
    }

    let missing = |key: crate::secrets::SecretKey| !matches!(keyring.get(key), Ok(Some(_)));

    if missing(crate::secrets::SecretKey::B2KeyId) || missing(crate::secrets::SecretKey::B2AppKey) {
        return Err(SyncError::Config(
            "密钥环里还没有 B2 凭据，请先在设置页里填写".to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::testing::settings;

    #[test]
    fn refuses_incomplete_settings() {
        let mut config = settings();
        config.enabled = false;
        assert!(matches!(validate(&config), Err(SyncError::NotEnabled)));

        let mut config = settings();
        config.bucket = String::new();
        assert!(
            validate(&config)
                .unwrap_err()
                .to_string()
                .contains("bucket")
        );

        // The endpoint is optional: a plain B2 setup needs only a bucket.
        let mut config = settings();
        config.endpoint = String::new();
        assert!(validate(&config).is_ok());

        // But a value that the native B2 backend cannot use is refused, with an
        // explanation — the S3 endpoint is what the B2 console shows first, and
        // sending it to the B2 API only produces a mystery 404.
        let mut config = settings();
        config.endpoint = "s3.us-west-004.backblazeb2.com".to_string();
        let error = validate(&config).unwrap_err().to_string();
        assert!(error.contains("S3 兼容接口"), "{error}");
        assert!(error.contains("留空"), "{error}");

        // A bare host is not a URL either; rclone would not add a scheme.
        let mut config = settings();
        config.endpoint = "api001.backblazeb2.com".to_string();
        assert!(
            validate(&config)
                .unwrap_err()
                .to_string()
                .contains("https://")
        );

        let mut config = settings();
        config.endpoint = "https://api001.backblazeb2.com".to_string();
        assert!(validate(&config).is_ok());

        // ⚠ 带协议头的 S3 端点同样要拦（BB-01）：那是 B2 控制台复制出来的最常见形态，
        // 而从前判断的是 `starts_with("s3.")` —— 加了 `https://` 就穿过去，于是"唯一
        // 被拦住的形态"恰好是不带协议的那个，与这段文案自己的要求正好相反。
        for value in [
            "https://s3.us-west-004.backblazeb2.com",
            "http://s3.us-west-004.backblazeb2.com",
            "  https://S3.US-WEST-004.BACKBLAZEB2.COM  ",
            "https://s3.us-west-004.backblazeb2.com/b2api/v3",
        ] {
            let mut config = settings();
            config.endpoint = value.to_string();
            let error = validate(&config).unwrap_err().to_string();
            assert!(error.contains("S3 兼容接口"), "{value} 该被拦下：{error}");
        }
    }

    // 假 secret-tool 是 shell 脚本,只在 Unix 上能跑(Windows 上 spawn 报
    // os error 193);其余的校验测试不 spawn 任何东西。
    #[cfg(unix)]
    #[test]
    fn missing_secrets_are_reported_before_a_run() {
        use crate::secrets::testing::FakeTool;
        use crate::secrets::{Keyring, SecretKey};

        let fake = FakeTool::new("sync-secrets");
        let keyring: Keyring = fake.keyring();

        let error = validate_secrets(&keyring, true).unwrap_err();
        assert!(error.to_string().contains("B2 凭据"), "{error}");

        keyring.set(SecretKey::B2KeyId, "id").unwrap();
        assert!(
            validate_secrets(&keyring, true).is_err(),
            "半个凭据不算有凭据"
        );
        keyring.set(SecretKey::B2AppKey, "key").unwrap();
        assert!(validate_secrets(&keyring, true).is_ok());
    }

    /// kopia 钉在本地目录仓库上时一个 B2 密码都用不上（SC-08）：从前这里无条件
    /// 索要 B2 凭据，于是那种配置在设置页里被引去填一段根本用不着的东西。
    ///
    /// 注意锁定态**仍然**优先：`validate_secrets` 里那句 `if !needs_b2` 在解锁检查
    /// 之后，锁着就是锁着，不会因为"这次用不上 B2"就说一切正常。
    #[test]
    fn a_local_repository_needs_no_b2_credentials() {
        use crate::secrets::Keyring;
        use crate::secrets::testing::FakeTool;

        let fake = FakeTool::new("sync-secrets-local");
        let keyring: Keyring = fake.keyring();
        assert!(
            validate_secrets(&keyring, false).is_ok(),
            "本地仓库不碰 B2，凭据一个都不用"
        );
        // 但 rclone 那条路只走 B2，判据不能把它的要求一起放开。
        let mut config = settings();
        config.engine = crate::config::SyncEngine::Rclone;
        assert!(needs_b2_credentials(&config), "rclone 一直只走 B2");
    }
}
