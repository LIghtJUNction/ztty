use ztty::scroll_prompt::{contains, place};

#[test]
fn prompt_avoids_search_and_palette_without_changing_the_viewport() {
    let plain = place(940, 664, 20, 24, 140, 0).unwrap();
    assert_eq!((plain.x + plain.width, plain.y + plain.height), (920, 644));
    for rows in [1, 4, 9] {
        let modal = place(940, 664, 20, 24, 140, rows).unwrap();
        assert_eq!(modal.x, plain.x);
        assert!(modal.y + modal.height < 664 - 20 - rows as i32 * 24);
    }
}

#[test]
fn narrow_and_short_views_require_a_complete_label_and_target() {
    assert!(place(80, 200, 20, 24, 140, 0).is_none());
    let compact = place(80, 200, 20, 24, 9, 0).unwrap();
    assert!(compact.x >= 20 && compact.x + compact.width <= 60);
    assert!(place(940, 60, 20, 24, 140, 1).is_none());
    assert!(place(940, 200, 20, 24, 140, usize::MAX).is_none());
    assert!(place(i32::MAX, i32::MAX, 20, 24, i32::MAX, 0).is_none());
}

#[test]
fn only_visible_target_pixels_are_clickable() {
    let rect = place(940, 664, 20, 24, 140, 0).unwrap();
    assert!(contains(Some(rect), rect.x, rect.y));
    assert!(contains(
        Some(rect),
        rect.x + rect.width - 1,
        rect.y + rect.height - 1
    ));
    assert!(!contains(Some(rect), rect.x - 1, rect.y));
    assert!(!contains(Some(rect), rect.x + rect.width, rect.y));
    assert!(!contains(Some(rect), rect.x, rect.y + rect.height));
    assert!(!contains(None, rect.x, rect.y));
}
