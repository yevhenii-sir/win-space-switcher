use windows::Win32::Foundation::{POINT, RECT, SIZE};

use crate::settings::PanelPosition;

/// Top-left corner for a panel of `size` inside the screen's `work` area, in physical pixels.
/// `gap` separates the panel from the screen edge or from the cursor.
pub fn panel_origin(position: PanelPosition, work: RECT, size: SIZE, gap: i32, cursor: POINT) -> POINT {
    let centered_y = work.top + (work.bottom - work.top - size.cy) / 2;
    match position {
        PanelPosition::RightEdge => POINT { x: work.right - size.cx - gap, y: centered_y },
        PanelPosition::ScreenCenter => POINT { x: work.left + (work.right - work.left - size.cx) / 2, y: centered_y },
        PanelPosition::NearCursor | PanelPosition::FollowCursor => {
            // Below-right of the pointer, flipped to the other side where it would not fit.
            let x = if cursor.x + gap + size.cx <= work.right { cursor.x + gap } else { cursor.x - gap - size.cx };
            let y = if cursor.y + gap + size.cy <= work.bottom { cursor.y + gap } else { cursor.y - gap - size.cy };
            POINT { x: keep_inside(x, work.left, work.right - size.cx), y: keep_inside(y, work.top, work.bottom - size.cy) }
        }
    }
}

/// Like `clamp`, but tolerates a panel larger than the area (then aligns it to the start).
fn keep_inside(value: i32, min: i32, max: i32) -> i32 {
    value.min(max).max(min)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: RECT = RECT { left: 0, top: 0, right: 1000, bottom: 800 };
    const SIZE_: SIZE = SIZE { cx: 200, cy: 100 };
    const GAP: i32 = 16;

    fn origin(position: PanelPosition, cursor: (i32, i32)) -> (i32, i32) {
        let p = panel_origin(position, WORK, SIZE_, GAP, POINT { x: cursor.0, y: cursor.1 });
        (p.x, p.y)
    }

    #[test]
    fn right_edge_is_vertically_centered_with_gap() {
        assert_eq!(origin(PanelPosition::RightEdge, (0, 0)), (784, 350));
    }

    #[test]
    fn screen_center() {
        assert_eq!(origin(PanelPosition::ScreenCenter, (0, 0)), (400, 350));
    }

    #[test]
    fn near_cursor_goes_below_right() {
        assert_eq!(origin(PanelPosition::NearCursor, (100, 100)), (116, 116));
    }

    #[test]
    fn near_cursor_flips_at_the_bottom_right_corner() {
        assert_eq!(origin(PanelPosition::NearCursor, (950, 780)), (734, 664));
    }

    #[test]
    fn near_cursor_stays_inside_offset_screens() {
        let work = RECT { left: -1920, top: 0, right: 0, bottom: 1080 };
        let p = panel_origin(PanelPosition::NearCursor, work, SIZE_, GAP, POINT { x: -5, y: 500 });
        assert_eq!((p.x, p.y), (-221, 516));
    }
}
