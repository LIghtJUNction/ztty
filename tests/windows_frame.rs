extern crate alloc;
#[path = "../src/platform/windows_frame.rs"]
mod frame;
use ztty::layout::Rect;

fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

#[test]
fn moved_held_frame_keeps_committed_pixels_and_does_not_touch_neighbours() {
    let source: Vec<u32> = (0..60).collect();
    let snapshot = frame::Snapshot::capture(&source, 10, rect(2, 1, 3, 2), 6).unwrap();
    let mut target = vec![100; 60];
    snapshot.blit(&mut target, 10, rect(6, 3, 3, 2));
    assert_eq!(snapshot.pixel_count(), 6);
    for row in 0..6 {
        for col in 0..10 {
            let expected = if (3..5).contains(&row) && (6..9).contains(&col) {
                source[(row - 2) * 10 + col - 4]
            } else {
                100
            };
            assert_eq!(target[row * 10 + col], expected);
        }
    }
}

#[test]
fn repeated_resize_uses_the_original_committed_frame() {
    let source: Vec<u32> = (0..24).collect();
    let snapshot = frame::Snapshot::capture(&source, 6, rect(1, 1, 4, 2), 8).unwrap();
    let mut small = vec![99; 8];
    snapshot.blit(&mut small, 4, rect(1, 0, 2, 1));
    assert_eq!(small, vec![99, 7, 8, 99, 99, 99, 99, 99]);
    let mut restored = vec![99; 24];
    snapshot.blit(&mut restored, 6, rect(1, 1, 4, 2));
    assert_eq!(&restored[7..11], &source[7..11]);
    assert_eq!(&restored[13..17], &source[13..17]);
}

#[test]
fn offscreen_capture_and_target_clipping_do_not_copy_outside_either_view() {
    let source: Vec<u32> = (0..12).collect();
    let snapshot = frame::Snapshot::capture(&source, 4, rect(-2, -1, 5, 3), 6).unwrap();
    let mut target = vec![99; 24];
    snapshot.blit(&mut target, 6, rect(0, 0, 5, 3));
    assert_eq!(&target[8..11], &source[0..3]);
    assert_eq!(&target[14..17], &source[4..7]);
    assert_eq!(target[7], 99);
    assert_eq!(target[11], 99);
    assert_eq!(target[17], 99);
}

#[test]
fn aggregate_budget_rejection_keeps_the_source_untouched() {
    let source = vec![0x123456; 20];
    let first = frame::Snapshot::capture(&source, 5, rect(0, 0, 3, 4), 20).unwrap();
    assert_eq!(first.pixel_count(), 12);
    assert!(
        frame::Snapshot::capture(&source, 5, rect(0, 0, 3, 4), 20 - first.pixel_count()).is_err()
    );
    assert!(frame::Snapshot::capture(&source, 0, rect(0, 0, 1, 1), 20).is_err());
    assert_eq!(source, vec![0x123456; 20]);
}
