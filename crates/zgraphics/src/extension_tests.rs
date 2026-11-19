use super::*;
use crate::tests::{base64, command, context, send};
use alloc::vec;

fn image(g: &mut Graphics, id: u32) {
    assert!(has(
        send(
            g,
            format!("Ga=T,i={id},p=1,s=2,v=1,C=1").as_bytes(),
            &[255, 0, 0, 255, 0, 255, 0, 255],
            context()
        ),
        b";OK"
    ));
}
fn has(reply: Vec<u8>, marker: &[u8]) -> bool {
    reply.windows(marker.len()).any(|bytes| bytes == marker)
}
#[test]
fn external_reader_receives_exact_range_and_only_runs_at_st() {
    fn reader(r: ExternalRead<'_>) -> Result<Vec<u8>, ReadError> {
        assert_eq!(
            (r.medium, r.name, r.offset, r.size),
            (Medium::File, b"/tmp/a".as_slice(), 7, 4)
        );
        assert!(r.limit >= 4);
        Ok(vec![11, 22, 33, 255])
    }
    let mut g = Graphics::default();
    g.set_external_reader(Some(reader));
    let mut stream = CommandStream::new();
    let mut replies = Vec::new();
    stream.push(
        &mut g,
        b"Ga=T,i=3,s=1,v=1,t=f,O=7,S=4;L3RtcC9h",
        context(),
        &mut replies,
    );
    assert!(replies.is_empty());
    assert_eq!(g.image_count(), 0);
    stream.finish(&mut g, context(), &mut replies);
    assert_eq!(replies, b"\x1b_Gi=3;OK\x1b\\");
    assert_eq!(g.image(3).unwrap().pixels.as_slice(), [11, 22, 33, 255]);
}
#[test]
fn external_failure_is_opaque_and_preserves_previous_image() {
    let mut g = Graphics::default();
    image(&mut g, 1);
    let before = Arc::clone(&g.image(1).unwrap().pixels);
    g.set_external_reader(Some(|_| Err(ReadError)));
    for transport in *b"fts" {
        assert_eq!(
            send(
                &mut g,
                format!("Ga=T,i=1,s=2,v=1,t={}", transport as char).as_bytes(),
                b"unknown",
                context()
            ),
            b"\x1b_Gi=1;EBADF:Failed to read image file\x1b\\"
        );
        assert!(Arc::ptr_eq(&before, &g.image(1).unwrap().pixels));
    }
    g.set_external_reader(Some(|_| Ok(vec![0; 9])));
    assert!(has(
        send(&mut g, b"Ga=T,i=1,s=2,v=1,t=f,S=8", b"a", context()),
        b"EBADF"
    ));
}
#[test]
fn external_feature_is_advertised_only_with_a_host_reader() {
    let mut g = Graphics::default();
    assert_eq!(
        command(&mut g, b"Ga=q,i=1,e=T,t=s"),
        b"\x1b_Gi=1,e=T;OK\x1b\\"
    );
    g.set_external_reader(Some(|_| Err(ReadError)));
    assert_eq!(
        command(&mut g, b"Ga=q,i=1,e=T,t=s"),
        b"\x1b_Gi=1,e=T,t=s;OK\x1b\\"
    );
}
#[test]
fn relative_chain_moves_once_and_never_moves_the_cursor() {
    let mut g = Graphics::default();
    for id in 1..=3 {
        image(&mut g, id);
    }
    command(&mut g, b"Ga=p,i=2,p=1,P=1,Q=1,H=2,V=3");
    command(&mut g, b"Ga=p,i=3,p=1,P=2,Q=1,H=-1,V=-2");
    assert_eq!(
        g.draws(0).find(|d| d.image_id == 3).unwrap().dest,
        PixelRect {
            x: 8,
            y: 16,
            width: 2,
            height: 1
        }
    );
    g.scroll(0, 23, -1, 1, true);
    assert_eq!(g.draws(1).find(|d| d.image_id == 3).unwrap().dest.y, 16);
    assert!(
        g.command(b"Ga=p,i=3,p=1,P=2,Q=1", context(), &mut Vec::new())
            .cursor_move
            .is_none()
    );
}
#[test]
fn relative_cycles_depth_and_replacement_descendants_are_rejected() {
    let mut g = Graphics::default();
    for id in 1..=18 {
        image(&mut g, id);
    }
    assert!(has(
        command(&mut g, b"Ga=p,i=1,p=1,P=100,Q=1"),
        b"ENOPARENT"
    ));
    assert!(has(command(&mut g, b"Ga=p,i=1,p=1,P=1,Q=1"), b"ECYCLE"));
    for id in 2..=17 {
        assert!(has(
            command(
                &mut g,
                format!("Ga=p,i={id},p=1,P={},Q=1", id - 1).as_bytes()
            ),
            b";OK"
        ));
    }
    assert!(has(command(&mut g, b"Ga=p,i=18,p=1,P=17,Q=1"), b"ETOODEEP"));
    assert!(has(command(&mut g, b"Ga=p,i=1,p=1,P=18,Q=1"), b"ETOODEEP"));
    assert!(has(command(&mut g, b"Ga=p,i=1,p=1,P=2,Q=1"), b"ECYCLE"));
    assert!(has(
        command(&mut g, b"Ga=p,i=18,p=1,P=1,Q=1,U=1"),
        b"EINVAL"
    ));
}
#[test]
fn parent_deletion_cascades_relative_placements_and_unreferenced_children() {
    let mut g = Graphics::default();
    for id in 1..=3 {
        image(&mut g, id);
    }
    command(&mut g, b"Ga=p,i=2,p=1,P=1,Q=1");
    command(&mut g, b"Ga=p,i=3,p=1,P=2,Q=1");
    command(&mut g, b"Ga=d,d=i,i=1,p=1");
    assert_eq!(g.placement_count(), 0);
    assert!(g.image(1).is_some());
    assert!(g.image(2).is_none() && g.image(3).is_none());
    assert_eq!(g.image_bytes(), 8);
}
#[test]
fn virtual_placement_clips_text_tiles_and_anchors_relative_children() {
    let mut g = Graphics::default();
    image(&mut g, 1);
    image(&mut g, 2);
    command(&mut g, b"Ga=p,i=1,p=1,U=1,c=2,r=1");
    command(&mut g, b"Ga=p,i=2,p=1,P=1,Q=1,H=1,V=1");
    assert_eq!(g.draws(0).count(), 0);
    let cells = [Placeholder {
        image_id: 1,
        placement_id: 1,
        col: 4,
        row: 3,
        image_col: 0,
        image_row: 0,
        columns: 2,
    }];
    let draws = g.draws_with_placeholders(0, &cells);
    let first = draws.iter().find(|d| d.image_id == 1).unwrap();
    assert_eq!(
        first.clip,
        Some(PixelRect {
            x: 32,
            y: 48,
            width: 16,
            height: 16
        })
    );
    assert_eq!((first.dest.x, first.dest.width), (32, 16));
    let child = draws.iter().find(|d| d.image_id == 2).unwrap();
    assert_eq!((child.dest.x, child.dest.y), (40, 64));
    command(&mut g, b"Ga=d,d=a");
    assert!(g.has_virtual_placements());
    command(&mut g, b"Ga=d,d=I,i=1");
    assert_eq!(g.image_count(), 0);
}
#[test]
fn partial_animation_frame_blends_and_replaces_without_mutating_base() {
    let mut g = Graphics::default();
    image(&mut g, 1);
    let base = Arc::clone(&g.image(1).unwrap().pixels);
    assert!(has(
        send(
            &mut g,
            b"Ga=f,i=1,s=1,v=1,x=1,c=1,z=20",
            &[0, 0, 255, 128],
            context()
        ),
        b";OK"
    ));
    assert_eq!(g.image_bytes(), 16);
    command(&mut g, b"Ga=a,i=1,c=2");
    assert_eq!(&g.image(1).unwrap().pixels[4..], [0, 127, 128, 255]);
    assert_eq!(base.as_slice(), [255, 0, 0, 255, 0, 255, 0, 255]);
    send(
        &mut g,
        b"Ga=f,i=1,s=1,v=1,x=0,r=2,X=1",
        &[7, 8, 9, 0],
        context(),
    );
    assert_eq!(&g.image(1).unwrap().pixels[..4], [7, 8, 9, 0]);
    assert_eq!(g.frame_count(1), 2);
}
#[test]
fn all_frames_share_quota_and_failed_frame_keeps_existing_state() {
    let mut g = Graphics::new(Limits {
        max_total_bytes: 16,
        max_image_bytes: 16,
        ..Limits::default()
    });
    image(&mut g, 1);
    send(&mut g, b"Ga=f,i=1,s=2,v=1", &[0; 8], context());
    assert_eq!(g.image_bytes(), 16);
    assert!(has(
        send(&mut g, b"Ga=f,i=1,s=2,v=1", &[0; 8], context()),
        b"ENOSPC"
    ));
    assert!(has(
        send(&mut g, b"Ga=f,i=1,s=1,v=1,x=2", &[0; 4], context()),
        b"EINVAL"
    ));
    assert_eq!(g.frame_count(1), 2);
    command(&mut g, b"Ga=d,d=f,i=1,r=2");
    assert_eq!((g.frame_count(1), g.image_bytes()), (1, 8));
}
#[test]
fn finite_animation_skips_gapless_root_and_stops_on_last_frame() {
    let mut g = Graphics::default();
    image(&mut g, 1);
    send(&mut g, b"Ga=f,i=1,s=2,v=1,z=10", &[9; 8], context());
    command(&mut g, b"Ga=a,i=1,s=3,v=2");
    assert_eq!(g.advance_animations(100, &[1]), Some(110));
    assert_eq!(g.image(1).unwrap().pixels.as_slice(), [9; 8]);
    assert_eq!(g.advance_animations(110, &[1]), None);
    assert!(!g.has_running_animations());
}
#[test]
fn hidden_loading_and_gapless_animations_leave_no_busy_timer() {
    let mut g = Graphics::default();
    image(&mut g, 1);
    command(&mut g, b"Ga=a,i=1,s=3");
    assert_eq!(g.advance_animations(10, &[1]), None);
    send(&mut g, b"Ga=f,i=1,s=2,v=1,z=10", &[9; 8], context());
    command(&mut g, b"Ga=a,i=1,s=2");
    assert_eq!(g.advance_animations(20, &[1]), Some(30));
    assert_eq!(g.advance_animations(30, &[1]), None);
    send(&mut g, b"Ga=f,i=1,s=2,v=1,z=10", &[7; 8], context());
    assert_eq!(g.image(1).unwrap().pixels.as_slice(), [7; 8]);
    assert_eq!(g.advance_animations(40, &[1]), Some(50));
    assert_eq!(g.advance_animations(45, &[]), None);
    assert_eq!(g.animation_deadline(), None);
    g.clear();
    assert_eq!(g.advance_animations(u64::MAX - 1, &[1]), None);
}
#[test]
fn frame_composition_checks_overlap_bounds_and_original_pixels() {
    let mut g = Graphics::default();
    image(&mut g, 1);
    send(&mut g, b"Ga=f,i=1,s=2,v=1", &[0; 8], context());
    assert!(has(
        command(&mut g, b"Ga=c,i=1,r=1,c=2,w=1,h=1,x=1,C=1"),
        b";OK"
    ));
    command(&mut g, b"Ga=a,i=1,c=2");
    assert_eq!(&g.image(1).unwrap().pixels[4..], [255, 0, 0, 255]);
    assert!(has(
        command(&mut g, b"Ga=c,i=1,r=2,c=2,w=1,h=1,x=1,X=1"),
        b"EINVAL"
    ));
    assert!(has(command(&mut g, b"Ga=c,i=1,r=4,c=2"), b"ENOENT"));
    assert_eq!(g.image_bytes(), 16);
}
#[test]
fn streaming_frame_commits_only_at_final_st() {
    let mut g = Graphics::default();
    image(&mut g, 1);
    let mut stream = CommandStream::new();
    let mut replies = Vec::new();
    let body = [b"Ga=f,i=1,s=2,v=1;".as_slice(), base64(&[42; 8]).as_slice()].concat();
    for byte in body {
        stream.push(&mut g, &[byte], context(), &mut replies);
    }
    assert_eq!(g.frame_count(1), 1);
    stream.finish(&mut g, context(), &mut replies);
    assert_eq!(g.frame_count(1), 2);
    assert!(has(replies, b";OK"));
}

#[test]
fn static_upload_cannot_exceed_scene_frame_count() {
    let mut g = Graphics::default();
    image(&mut g, 1);
    for _ in 1..512 {
        assert!(has(
            send(&mut g, b"Ga=f,i=1,s=2,v=1", &[0; 8], context()),
            b";OK"
        ));
    }
    assert_eq!(g.total_frames(), 512);
    assert!(has(
        send(&mut g, b"Ga=T,i=2,s=1,v=1", &[1, 2, 3, 255], context()),
        b"ENOSPC"
    ));
    assert_eq!(g.total_frames(), 512);
    assert!(g.image(2).is_none());
    command(&mut g, b"Ga=d,d=f,i=1,r=512");
    assert!(has(
        send(&mut g, b"Ga=T,i=2,s=1,v=1", &[1, 2, 3, 255], context()),
        b";OK"
    ));
    assert_eq!(g.total_frames(), 512);
}
#[test]
fn running_animation_at_saturated_clock_has_no_expired_wake() {
    let mut g = Graphics::default();
    image(&mut g, 1);
    send(&mut g, b"Ga=f,i=1,s=2,v=1,z=10", &[9; 8], context());
    command(&mut g, b"Ga=a,i=1,s=3");
    assert_eq!(g.advance_animations(u64::MAX, &[1]), None);
    assert_eq!(g.animation_deadline(), None);
    assert_eq!(g.advance_animations(u64::MAX, &[1]), None);
}

#[test]
fn feature_queries_report_action_specific_values_without_false_capabilities() {
    let mut g = Graphics::default();
    for (query, expected) in [
        (
            b"Ga=q,i=1,e=a,s=4,v=3,c=2,r=1,z=-1,S=9,p=1,X=1,w=1,U=1".as_slice(),
            b"\x1b_Gi=1,e=a,v=3,c=2,r=1,z=-1;OK\x1b\\".as_slice(),
        ),
        (
            b"Ga=q,i=1,e=f,f=24,c=1,r=2,X=2,Y=17,w=1,U=1,C=1",
            b"\x1b_Gi=1,e=f,f=24,c=1,r=2,Y=17;OK\x1b\\",
        ),
        (
            b"Ga=q,i=1,e=q,f=100,s=4,t=s,U=1,C=1;ignored",
            b"\x1b_Gi=1,e=q,f=100,s=4;OK\x1b\\",
        ),
    ] {
        assert_eq!(command(&mut g, query), expected);
    }
    for format in [8, 16, 32] {
        assert_eq!(
            command(&mut g, format!("Ga=q,i=1,e=T,f={format}").as_bytes()),
            b"\x1b_Gi=1,e=T;OK\x1b\\"
        );
    }
    assert_eq!(g.image_count(), 0);
}

#[test]
fn external_short_pixels_and_invalid_png_use_the_same_opaque_file_error() {
    let mut g = Graphics::default();
    image(&mut g, 1);
    let before = Arc::clone(&g.image(1).unwrap().pixels);
    g.set_external_reader(Some(|_| Ok(vec![0; 4])));
    for header in [b"Ga=T,i=1,s=2,v=1,t=f".as_slice(), b"Ga=T,i=1,f=100,t=f"] {
        assert_eq!(
            send(&mut g, header, b"/synthetic", context()),
            b"\x1b_Gi=1;EBADF:Failed to read image file\x1b\\"
        );
        assert!(Arc::ptr_eq(&before, &g.image(1).unwrap().pixels));
    }
}

#[test]
fn anchor_reflow_moves_only_physical_roots_and_cascades_lost_anchors() {
    let mut g = Graphics::default();
    for id in 1..=4 {
        image(&mut g, id);
    }
    command(&mut g, b"Ga=p,i=2,p=1,P=1,Q=1,H=2,V=3");
    command(&mut g, b"Ga=p,i=3,p=1,U=1,c=2,r=1");
    command(&mut g, b"Ga=p,i=4,p=1,P=3,Q=1,H=1,V=1");
    assert_eq!(g.anchors().collect::<Vec<_>>(), [(0, 0)]);
    let before = g.draws(0).find(|d| d.image_id == 2).unwrap().dest;
    let mut mapped = 0;
    g.remap_anchors(|col, row| {
        mapped += 1;
        Some((col + 3, row + 2))
    });
    assert_eq!(mapped, 1);
    assert_eq!(g.anchors().collect::<Vec<_>>(), [(3, 2)]);
    let after = g.draws(0).find(|d| d.image_id == 2).unwrap().dest;
    assert_eq!((after.x - before.x, after.y - before.y), (24, 32));
    g.remap_anchors(|_, _| None);
    assert_eq!(g.anchors().count(), 0);
    assert_eq!(g.placement_count(), 2); // Virtual root and its dependent remain.
    assert!(g.image(2).is_none() && g.image(3).is_some() && g.image(4).is_some());
}
