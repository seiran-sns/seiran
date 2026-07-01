use std::sync::atomic::{AtomicU32, Ordering};
use chrono::{DateTime, Utc};

static SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// 48bitのタイムスタンプと16bitのシリアル値によるソート可能な64bit一意ID (Snowflake) を生成します。
/// 外部プロトコルの投稿時刻（`created_at`）が自サーバーの現在時刻よりも未来を指している場合、
/// タイムラインのUX破壊を防ぐために、現在時刻をタイムスタンプ部分に適用する（未来補正）。
pub fn generate_snowflake_id(created_at: DateTime<Utc>) -> i64 {
    let now = Utc::now();
    
    // 未来補正アルゴリズム
    let target_time = if created_at > now {
        now
    } else {
        created_at
    };
    
    let timestamp_ms = target_time.timestamp_millis();
    
    // 48bitマスク (48bitは西暦10000年以降までミリ秒を表現可能です)
    let masked_timestamp = (timestamp_ms & 0x0000_FFFF_FFFF_FFFF) as u64;
    
    // 衝突防止用のシリアル値 (16bitマスクで最大65,535ミリ秒内衝突を防ぐ)
    let seq = SEQUENCE.fetch_add(1, Ordering::Relaxed) & 0xFFFF;
    
    // 64bitにパッキング (前半48bit タイムスタンプ | 後半16bit シリアル値)
    let snowflake = (masked_timestamp << 16) | (seq as u64);
    
    snowflake as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_snowflake_sorting() {
        let now = Utc::now();
        let id1 = generate_snowflake_id(now);
        let id2 = generate_snowflake_id(now + Duration::milliseconds(1));
        
        assert!(id1 < id2, "ID should be chronologically sortable");
    }

    #[test]
    fn test_future_correction() {
        let far_future = Utc::now() + Duration::days(1);
        let id = generate_snowflake_id(far_future);
        
        // 生成されたIDのタイムスタンプを復元
        let restored_ts_ms = ((id as u64) >> 16) as i64;
        let now_ms = Utc::now().timestamp_millis();
        
        // 未来の時間ではなく、概ね現在時刻に補正されていることを検証
        assert!((restored_ts_ms - now_ms).abs() < 1000, "Future timestamp should be corrected to approximately now");
    }
}
