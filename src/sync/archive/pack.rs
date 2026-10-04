//! 打包：把 [`gather`] 收来的东西写成一个 zip，并在包根放一份清单。
//!
//! 只做"装进去"这一半：要不要覆盖、谁更新，全在 `unpack` 那边——那边才是会动
//! 本机数据的地方。收集规则在 `gather`，与 kopia 那条路（摆成目录）共用一份。

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::digest::digest_of;
use super::gather::gather;
use super::{Entry, FORMAT, MANIFEST, Manifest};
use crate::sync::SaveTarget;
use crate::sync::cloud::PackIdentity;

/// 打包过程中攒下来的东西：一个包的内容，以及它没能包含谁。
#[derive(Debug, Clone)]
pub struct PackReport {
    /// 进了包的文件，按存档位置和相对路径排好序。
    pub entries: Vec<Entry>,
    /// 这一版包含哪些存档位置（目录在，哪怕空着）。
    pub locations: Vec<String>,
    /// 本机根本没有的存档位置。报告成 `skipped`，不是错误：一台机器上没装
    /// 某个位置（只在 Windows 上才有的那个目录）是正常的。
    pub missing: Vec<String>,
    /// 打包时被排除规则挡下的文件数，只用于日志。
    pub excluded: usize,
    /// 这一版的**内容值**（[`super::digest`]），与写进包清单的是同一个值。
    ///
    /// 上传成功后要拿它写索引的 `latest_digest`（PLATFORMS.md §6.6 第 1 步）：
    /// 打包时文件已经读过一遍，顺手量出来的哈希直接算成它 —— 不必为了这个值
    /// 再读一遍盘，而且写进索引的就是**真的上去了的那一版**。
    pub digest: String,
}

/// 把 `targets` 里存在的每个存档位置打进 `zip_path`。
///
/// `now` 只用来写清单里的 `created`，由调用方传进来，测试才好断言。
/// `identity` 是**这一版是谁传的**：它有值，取回时才有资格比对；没有就如实留空
/// （那种包在自动取回那条路上会被拒绝，见 [`crate::sync::cloud::identity_match`]）。
pub fn pack(
    zip_path: &Path,
    targets: &[SaveTarget],
    now: chrono::DateTime<chrono::Utc>,
    identity: Option<&PackIdentity>,
) -> Result<PackReport, String> {
    let gathered = gather(targets)?;
    // `digest` 先留空：它得看文件的每一个字节，而那些字节正好在这一趟"装进包"里
    // 读过一遍（下面循环），所以两件事共用同一次读盘（§6.6 第 1 步）。
    let mut manifest = Manifest {
        format: FORMAT,
        created: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        locations: gathered.locations,
        identity: identity.cloned(),
        digest: None,
        entries: gathered.entries,
    };
    // `(位置内的相对路径, 全文哈希)`：digest 的原料，与包里的内容是同一批字节。
    let mut hashes: Vec<(String, String)> = Vec::with_capacity(gathered.files.len());

    let file = File::create(zip_path)
        .map_err(|e| format!("无法创建存档包 {}: {e}", zip_path.display()))?;
    let mut writer = ZipWriter::new(file);
    // deflate：Windows 资源管理器双击就能打开，压缩率也够。加密在 zip 这条路上
    // 不存在——想保护就选 kopia。
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    for (key, absolute, relative) in &gathered.files {
        let name = format!("{key}/{relative}");
        // 读一遍：同一份字节既写进包，又喂给 digest 的哈希。整份读进内存是刻意的
        // —— 存档文件不大，而这样"写出去的字节"与"哈希覆盖的字节"永远是同一份；
        // 流式读的话两者得靠 `io::copy` 的返回值对齐，多一处出错的机会。
        let bytes = read_file(absolute)?;
        hashes.push((
            relative.clone(),
            crate::sync::fingerprint::hash_bytes(&bytes),
        ));

        writer
            .start_file(name.clone(), options)
            .map_err(|e| format!("打包 {name} 失败: {e}"))?;
        writer
            .write_all(&bytes)
            .map_err(|e| format!("写入 {name} 失败: {e}"))?;
    }

    let digest = digest_of(
        hashes
            .iter()
            .map(|(relative, hash)| (relative.as_str(), hash.as_str())),
    );
    manifest.digest = Some(digest.clone());

    let text =
        serde_json::to_string_pretty(&manifest).map_err(|e| format!("清单序列化失败: {e}"))?;
    writer
        .start_file(MANIFEST, options)
        .map_err(|e| format!("写入清单失败: {e}"))?;
    writer
        .write_all(text.as_bytes())
        .map_err(|e| format!("写入清单失败: {e}"))?;
    writer
        .finish()
        .map_err(|e| format!("收尾存档包 {} 失败: {e}", zip_path.display()))?;

    Ok(PackReport {
        entries: manifest.entries,
        locations: manifest.locations,
        missing: gathered.missing,
        excluded: gathered.excluded,
        digest,
    })
}

/// 整份读一个文件（存档都不大；包与 digest 要的是同一批字节）。
fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let mut bytes = Vec::with_capacity(size as usize);
    file.read_to_end(&mut bytes)
        .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    Ok(bytes)
}
