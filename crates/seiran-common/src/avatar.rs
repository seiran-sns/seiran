//! アバター未設定のローカル actor 向け決定論的 SVG アイコン。

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn seeded(actor_id: i64) -> u64 {
    actor_id
        .to_le_bytes()
        .into_iter()
        .fold(FNV_OFFSET, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
        })
}

/// actor ID から常に同じ代替アバター URL を組み立てる。
pub fn fallback_avatar_url(local_domain: &str, actor_id: i64) -> String {
    format!("https://{local_domain}/api/avatars/{actor_id}?v=2")
}

/// actor ID をシードに、背景・顔色・表情を決定論的に生成する。
pub fn fallback_avatar_svg(actor_id: i64) -> String {
    let seed = seeded(actor_id);
    let hue = seed % 360;
    let background_hue = (hue + 90) % 360;
    let horizontal = ((seed >> 9) % 3) as i32 - 1;
    let eye_y = 43 + (((seed >> 17) % 3) as i32 - 1) * 5;
    let eye_gap = 18 + ((seed >> 25) % 3) as i32 * 5;
    let center_x = 50 + horizontal * 9;
    let left_eye = center_x - eye_gap / 2;
    let right_eye = center_x + eye_gap / 2;
    let mouth = match (seed >> 33) % 6 {
        0 => format!(r#"<path d="M{} 65 H{}"/>"#, center_x - 13, center_x + 13),
        1 => format!(
            r#"<path d="M{} 61 Q{} 75 {} 61"/>"#,
            center_x - 14,
            center_x,
            center_x + 14
        ),
        2 => format!(
            r#"<path d="M{} 60 L{} 72 L{} 60"/>"#,
            center_x - 14,
            center_x,
            center_x + 14
        ),
        3 => format!(
            r#"<path d="M{} 58 V72 Q{} 65 {} 58 Z"/>"#,
            center_x - 14,
            center_x + 14,
            center_x - 14
        ),
        4 => format!(
            r#"<path d="M{} 61 Q{} 72 {} 61 Q{} 72 {} 61"/>"#,
            center_x - 16,
            center_x - 8,
            center_x,
            center_x + 8,
            center_x + 16
        ),
        _ => format!(
            r#"<path d="M{} 59 V69 M{} 64 H{} M{} 59 V69"/>"#,
            center_x - 14,
            center_x - 14,
            center_x + 14,
            center_x + 14
        ),
    };

    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><rect width="100" height="100" fill="hsl({background_hue} 65% 72%)"/><circle cx="50" cy="50" r="40" fill="hsl({hue} 65% 72%)"/><g fill="#000"><circle cx="{left_eye}" cy="{eye_y}" r="3"/><circle cx="{right_eye}" cy="{eye_y}" r="3"/></g><g fill="none" stroke="#000" stroke-width="4" stroke-linecap="round" stroke-linejoin="round" transform="translate({center_x} 65) scale(.8) translate(-{center_x} -65)">{mouth}</g></svg>"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_deterministic_and_varies_by_actor() {
        assert_eq!(fallback_avatar_svg(42), fallback_avatar_svg(42));
        assert_ne!(fallback_avatar_svg(42), fallback_avatar_svg(43));
    }

    #[test]
    fn url_uses_actor_id() {
        assert_eq!(
            fallback_avatar_url("example.com", 42),
            "https://example.com/api/avatars/42?v=2"
        );
    }
}
