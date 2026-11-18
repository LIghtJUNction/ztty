//! Geometry for a temporary scrollback affordance, outside modal overlays.
use crate::layout::Rect;

pub const INSET_X: i32 = 8;
pub const INSET_Y: i32 = 4;

/// Place a complete label above any bottom overlay, or omit it if it cannot fit.
/// The caller can retry with a shorter label without changing the hit target.
pub fn place(
    width: i32,
    height: i32,
    padding: i32,
    line_height: i32,
    label_width: i32,
    overlay_rows: usize,
) -> Option<Rect> {
    if width <= 0 || height <= 0 || line_height <= 0 || label_width <= 0 {
        return None;
    }
    let margin = padding.max(6);
    let badge_width = label_width.checked_add(INSET_X * 2)?;
    let badge_height = line_height.checked_add(INSET_Y * 2)?;
    let bottom = if overlay_rows == 0 {
        height.saturating_sub(margin)
    } else {
        let rows = i32::try_from(overlay_rows).ok()?;
        height
            .saturating_sub(padding.max(0))
            .saturating_sub(line_height.saturating_mul(rows))
            .saturating_sub(6)
    };
    let x = width.saturating_sub(margin).saturating_sub(badge_width);
    let y = bottom.saturating_sub(badge_height);
    (x >= margin && y >= margin).then_some(Rect {
        x,
        y,
        width: badge_width,
        height: badge_height,
    })
}

pub fn contains(rect: Option<Rect>, x: i32, y: i32) -> bool {
    rect.is_some_and(|rect| {
        x >= rect.x
            && y >= rect.y
            && x < rect.x.saturating_add(rect.width)
            && y < rect.y.saturating_add(rect.height)
    })
}
