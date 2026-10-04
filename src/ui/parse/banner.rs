//! 横幅那一份 `sync.snapshot` 回包 → [`SyncSnapshot`]（PLATFORMS.md §6.8 A）。
//!
//! 与 `parse::cloud` 分开：那一族说的是"云端清单"（一个桶一份的索引），这一份说的是
//! **本机 / 云端 / 基线三方现在各是什么** —— 它两边都读，所以不属于任何一边。
//!
//! ⚠ 这一层**只搬事实**，一个字都不拼：四行文案在 `ui::model::banner` 里（那样才测得住）。

use super::*;

pub(in crate::ui) fn parse_sync_snapshot(value: &Value) -> Result<SyncSnapshot, String> {
    let local = value
        .get("local")
        .ok_or_else(|| "回包里没有 local".to_string())?;
    let cloud = value
        .get("cloud")
        .ok_or_else(|| "回包里没有 cloud".to_string())?;
    let index = value.get("index").cloned().unwrap_or(Value::Null);

    let optional = |of: &Value, key: &str| -> Option<String> {
        of.get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };

    Ok(SyncSnapshot {
        game_id: str_field(value, "game_id"),
        local: SnapshotLocal {
            mtime_ms: local.get("mtime_ms").and_then(Value::as_i64).unwrap_or(0),
            empty: local.get("empty").and_then(Value::as_bool).unwrap_or(false),
            // ⚠ `null` 与"空串"都当**没算过**：内容值永远不可能是个空串
            // （`archive::digest_of` 给的是 64 位十六进制）。拿空串当"算出来是空的"
            // 会让横幅显示一个假的内容值。
            digest: optional(local, "digest"),
        },
        cloud: SnapshotCloud {
            state: str_field(cloud, "state"),
            stamp: optional(cloud, "stamp"),
            digest: optional(cloud, "digest"),
            key: str_field(cloud, "key"),
        },
        baseline: value
            .get("baseline")
            .and_then(Value::as_object)
            .map(|b| SnapshotBaseline {
                stamp: b
                    .get("stamp")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                digest: b
                    .get("digest")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                mtime_ms: b.get("mtime_ms").and_then(Value::as_i64).unwrap_or(0),
            }),
        decision: str_field(value, "decision"),
        index: SnapshotIndex {
            from_cache: index
                .get("from_cache")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            cached_at: str_field(&index, "cached_at"),
            stale: index.get("stale").and_then(Value::as_bool).unwrap_or(false),
            refresh_error: index
                .get("refresh_error")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_string),
        },
        problem: value
            .get("problem")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> Value {
        serde_json::json!({
            "game_id": "demo",
            "local": { "mtime_ms": 1759570000000i64, "empty": false, "digest": "aa".repeat(32) },
            "cloud": {
                "state": "known",
                "stamp": "20261004T101500Z",
                "digest": "aa".repeat(32),
                "key": "demo"
            },
            "baseline": { "stamp": "20261004T101500Z", "digest": "aa".repeat(32), "mtime_ms": 1759570000000i64 },
            "decision": "nothing",
            "index": {
                "from_cache": true,
                "cached_at": "20261005T101500Z",
                "stale": false,
                "refresh_error": null
            },
            "problem": null
        })
    }

    #[test]
    fn a_full_reply_parses_into_the_three_sides() {
        let snapshot = parse_sync_snapshot(&payload()).unwrap();
        assert_eq!(snapshot.game_id, "demo");
        assert_eq!(snapshot.local.mtime_ms, 1_759_570_000_000);
        assert!(!snapshot.local.empty);
        assert_eq!(
            snapshot.local.digest.as_deref(),
            Some("aa".repeat(32).as_str())
        );
        assert_eq!(snapshot.cloud.state, "known");
        assert_eq!(snapshot.decision, "nothing");
        let baseline = snapshot.baseline.expect("基线该在");
        assert_eq!(baseline.stamp, "20261004T101500Z");
        assert!(snapshot.index.from_cache);
        assert!(!snapshot.index.stale);
        assert!(snapshot.index.refresh_error.is_none());
        assert!(snapshot.problem.is_none());
    }

    /// ⚠ `null` / 空串 / 没有这一栏都当**没算过** —— 绝不能变成"内容值是空串"。
    #[test]
    fn a_missing_local_digest_means_we_have_not_checked() {
        let mut payload = payload();
        payload["local"]["digest"] = Value::Null;
        assert!(
            parse_sync_snapshot(&payload)
                .unwrap()
                .local
                .digest
                .is_none()
        );

        payload["local"]["digest"] = serde_json::json!("");
        assert!(
            parse_sync_snapshot(&payload)
                .unwrap()
                .local
                .digest
                .is_none()
        );

        payload["local"].as_object_mut().unwrap().remove("digest");
        assert!(
            parse_sync_snapshot(&payload)
                .unwrap()
                .local
                .digest
                .is_none()
        );
    }

    /// 没有基线（第一次同步）：`baseline` 是 `null`，不许当成"有一条空基线"。
    #[test]
    fn no_baseline_is_none_not_an_empty_one() {
        let mut payload = payload();
        payload["baseline"] = Value::Null;
        assert!(parse_sync_snapshot(&payload).unwrap().baseline.is_none());
    }

    /// 索引那一栏整块缺了也收着认（老 daemon）：横幅上少一句"什么时候拿的"，
    /// 总比整块报错强 —— 那四行里三行是真事实。
    #[test]
    fn a_reply_without_the_index_block_still_parses() {
        let mut payload = payload();
        payload.as_object_mut().unwrap().remove("index");
        let snapshot = parse_sync_snapshot(&payload).unwrap();
        assert!(snapshot.index.cached_at.is_empty());
        assert!(snapshot.index.refresh_error.is_none());
    }
}
