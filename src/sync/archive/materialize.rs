//! 摆成目录：把这一版的存档原样复制到一个目录树里，外加一份清单。
//!
//! 这是给 kopia 用的"打包"。kopia 快照的是**目录树**，而一个游戏的存档位置散在
//! 好几个地方，所以先照 zip 那条路的规矩摆成 `<key>/<相对路径>`，它才能一次拍下
//! 整个游戏。摆出来的东西和 zip 里的内容一一对应，清单也是同一份 [`Manifest`]，
//! 于是恢复那条路（`unpack`）两个引擎都能走。
//!
//! 代价是本地多一次读写（zip 那条路是直接读原文件压缩，这里要落一份副本：读进来
//! 顺手算 digest，再写出去）。换来的是 kopia 能对**未压缩的原始文件**做内容去重
//! ——喂给它 zip 的话，压缩后的字节几乎没有重复可找，去重就白搭了。

use std::path::Path;

use super::digest::digest_of;
use super::gather::gather;
use super::pack::PackReport;
use super::{FORMAT, MANIFEST, Manifest};
use crate::sync::SaveTarget;
use crate::sync::cloud::PackIdentity;

/// 把 `targets` 里存在的每个存档位置摆进 `dir`，并在 `dir` 根写一份清单。
///
/// `dir` 里的旧内容不会被清理：调用方给的总是一个刚建出来的空目录（`Staging`
/// 的临时目录用完即删），多一条"先清空"的路径只会多一个删错东西的机会。
/// `identity` 与 zip 那条路同一个意思（见 [`super::pack`]）。
pub fn materialize(
    dir: &Path,
    targets: &[SaveTarget],
    now: chrono::DateTime<chrono::Utc>,
    identity: Option<&PackIdentity>,
) -> Result<PackReport, String> {
    let gathered = gather(targets)?;
    // `digest` 先留空：真值在下面"复制时顺手算"里填进去 —— 每个文件只读一遍。
    let mut manifest = Manifest {
        format: FORMAT,
        created: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        locations: gathered.locations,
        identity: identity.cloned(),
        digest: None,
        entries: gathered.entries,
    };
    // `(位置内的相对路径, 全文哈希)`：与摆出来的那些文件是同一批字节。
    let mut hashes: Vec<(String, String)> = Vec::with_capacity(gathered.files.len());

    for (key, absolute, relative) in &gathered.files {
        let destination = dir
            .join(key)
            .join(relative.split('/').collect::<std::path::PathBuf>());
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("无法创建 {}: {e}", parent.display()))?;
        }
        // 读一遍：同一份字节既摆在目录里，又喂给 digest 的哈希。整份读进内存是
        // 刻意的 —— 存档文件不大，而这样"写出去的字节"与"哈希覆盖的字节"是同一份。
        let bytes = std::fs::read(absolute)
            .map_err(|e| format!("读取 {} 失败: {e}", absolute.display()))?;
        hashes.push((
            relative.clone(),
            crate::sync::fingerprint::hash_bytes(&bytes),
        ));
        std::fs::write(&destination, &bytes)
            .map_err(|e| format!("无法写入 {}: {e}", destination.display()))?;
    }

    let digest = digest_of(
        hashes
            .iter()
            .map(|(relative, hash)| (relative.as_str(), hash.as_str())),
    );
    manifest.digest = Some(digest.clone());

    let text =
        serde_json::to_string_pretty(&manifest).map_err(|e| format!("清单序列化失败: {e}"))?;
    std::fs::write(dir.join(MANIFEST), text)
        .map_err(|e| format!("写入清单 {} 失败: {e}", dir.join(MANIFEST).display()))?;

    Ok(PackReport {
        entries: manifest.entries,
        locations: manifest.locations,
        missing: gathered.missing,
        excluded: gathered.excluded,
        digest,
    })
}
