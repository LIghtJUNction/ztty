use super::*;
use alloc::vec;

fn context() -> Context {
    Context {
        col: 0,
        row: 0,
        cols: 80,
        rows: 24,
        cell_width: 8,
        cell_height: 16,
    }
}

fn base64(bytes: &[u8]) -> Vec<u8> {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    for c in bytes.chunks(3) {
        let a = c[0];
        let b = c.get(1).copied().unwrap_or(0);
        let d = c.get(2).copied().unwrap_or(0);
        out.push(TABLE[(a >> 2) as usize]);
        out.push(TABLE[(((a & 3) << 4) | (b >> 4)) as usize]);
        out.push(if c.len() > 1 {
            TABLE[(((b & 15) << 2) | (d >> 6)) as usize]
        } else {
            b'='
        });
        out.push(if c.len() > 2 {
            TABLE[(d & 63) as usize]
        } else {
            b'='
        });
    }
    out
}

fn send(g: &mut Graphics, header: &[u8], data: &[u8], ctx: Context) -> Vec<u8> {
    let mut body = header.to_vec();
    body.push(b';');
    body.extend(base64(data));
    let mut replies = Vec::new();
    g.command(&body, ctx, &mut replies);
    replies
}

fn command(g: &mut Graphics, body: &[u8]) -> Vec<u8> {
    let mut replies = Vec::new();
    g.command(body, context(), &mut replies);
    replies
}

#[test]
fn query_decodes_without_storing_or_replacing_an_image() {
    let mut g = Graphics::default();
    assert_eq!(
        send(
            &mut g,
            b"Ga=T,f=24,i=7,s=1,v=1,C=1",
            &[255, 0, 0],
            context()
        ),
        b"\x1b_Gi=7;OK\x1b\\"
    );
    let generation = g.image(7).unwrap().generation;
    assert_eq!(
        send(&mut g, b"Ga=q,f=24,i=7,s=1,v=1", &[0, 0, 255], context()),
        b"\x1b_Gi=7;OK\x1b\\"
    );
    assert_eq!(g.image(7).unwrap().pixels.as_slice(), [255, 0, 0, 255]);
    assert_eq!(g.image(7).unwrap().generation, generation);
    assert_eq!((g.image_count(), g.placement_count()), (1, 1));
}

#[test]
fn chunked_rgba_is_atomic_and_uses_final_cursor_position() {
    let mut g = Graphics::default();
    assert!(command(&mut g, b"Ga=T,i=2,s=2,v=1,m=1,C=1;/wAA").is_empty());
    assert_eq!((g.pending_bytes(), g.image_count()), (3, 0));
    let mut replies = Vec::new();
    g.command(
        b"Gm=0;/wD/AIA=",
        Context {
            col: 4,
            row: 3,
            ..context()
        },
        &mut replies,
    );
    assert_eq!(replies, b"\x1b_Gi=2;OK\x1b\\");
    assert_eq!(
        g.image(2).unwrap().pixels.as_slice(),
        [255, 0, 0, 255, 0, 255, 0, 128]
    );
    let d = g.draws(0).next().unwrap();
    assert_eq!((d.dest.x, d.dest.y), (32, 48));
    assert_eq!(g.pending_bytes(), 0);
}

#[test]
fn invalid_chunks_abort_but_leave_existing_images_untouched() {
    let mut g = Graphics::default();
    send(&mut g, b"Ga=T,i=2,s=1,v=1,C=1", &[1, 2, 3, 4], context());
    command(&mut g, b"Ga=T,i=2,s=2,v=1,m=1,C=1;/wAA");
    assert!(
        command(&mut g, b"Gm=0;!!!!")
            .windows(6)
            .any(|w| w == b"EINVAL")
    );
    assert_eq!(g.pending_bytes(), 0);
    assert_eq!(g.image(2).unwrap().pixels.as_slice(), [1, 2, 3, 4]);
    assert_eq!(g.placement_count(), 1);
    command(&mut g, b"Ga=T,i=3,s=2,v=1,m=1,C=1;/wAA");
    command(&mut g, b"Ga=d,d=i,i=9");
    assert_eq!(g.pending_bytes(), 0);
}

#[test]
fn png_formats_decode_to_straight_rgba() {
    for (file, expected) in [
        (
            include_bytes!("../tests/data/rgba.png").as_slice(),
            vec![
                255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 255, 255, 255, 255,
            ],
        ),
        (
            include_bytes!("../tests/data/rgb.png").as_slice(),
            vec![17, 34, 51, 255, 68, 85, 102, 255],
        ),
        (
            include_bytes!("../tests/data/gray.png").as_slice(),
            vec![16, 16, 16, 255, 240, 240, 240, 255],
        ),
        (
            include_bytes!("../tests/data/gray-alpha.png").as_slice(),
            vec![16, 16, 16, 255, 240, 240, 240, 64],
        ),
        (
            include_bytes!("../tests/data/palette.png").as_slice(),
            vec![255, 0, 0, 255, 0, 255, 0, 128],
        ),
        (
            include_bytes!("../tests/data/rgba16.png").as_slice(),
            vec![18, 86, 154, 222],
        ),
        (
            include_bytes!("../tests/data/adam7.png").as_slice(),
            vec![17, 34, 51, 68],
        ),
        (
            include_bytes!("../tests/data/metadata.png").as_slice(),
            vec![17, 34, 51, 68],
        ),
    ] {
        let mut g = Graphics::default();
        let reply = send(&mut g, b"Ga=T,i=1,f=100,C=1", file, context());
        assert_eq!(reply, b"\x1b_Gi=1;OK\x1b\\");
        assert_eq!(g.image(1).unwrap().pixels.as_slice(), expected);
    }
}

#[test]
fn invalid_png_crc_truncation_and_bombs_are_rejected() {
    let png = include_bytes!("../tests/data/rgba.png");
    let mut bad_crc = png.to_vec();
    bad_crc[29] ^= 1;
    for bad in [
        &bad_crc[..],
        &png[..png.len() - 1],
        include_bytes!("../tests/data/oversized.png").as_slice(),
        include_bytes!("../tests/data/inflate-bomb.png").as_slice(),
    ] {
        let mut g = Graphics::default();
        let reply = send(&mut g, b"Ga=T,i=1,f=100,C=1", bad, context());
        assert!(!reply.windows(3).any(|w| w == b";OK"));
        assert_eq!(
            (g.image_bytes(), g.image_count(), g.pending_bytes()),
            (0, 0, 0)
        );
    }
}

#[test]
fn compressed_rgb_decodes_and_wrong_dimensions_fail() {
    let mut g = Graphics::default();
    let data = include_bytes!("../tests/data/rgb.zlib");
    assert_eq!(
        send(&mut g, b"Ga=T,i=1,f=24,s=2,v=1,o=z,C=1", data, context()),
        b"\x1b_Gi=1;OK\x1b\\"
    );
    assert_eq!(
        g.image(1).unwrap().pixels.as_slice(),
        [17, 34, 51, 255, 68, 85, 102, 255]
    );
    let reply = send(&mut g, b"Ga=t,i=2,f=24,s=1,v=1,o=z", data, context());
    assert!(!reply.windows(3).any(|w| w == b";OK"));
    assert!(g.image(2).is_none());
}

#[test]
fn crop_aspect_ratio_letterboxing_and_metric_resize() {
    let mut g = Graphics::default();
    send(
        &mut g,
        b"Ga=T,i=1,s=4,v=2,x=1,w=2,h=2,c=4,r=4,C=1",
        &[255; 32],
        context(),
    );
    let d = g.draws(0).next().unwrap();
    assert_eq!(
        d.source,
        Rect {
            x: 1,
            y: 0,
            width: 2,
            height: 2
        }
    );
    assert_eq!(
        d.dest,
        PixelRect {
            x: 0,
            y: 16,
            width: 32,
            height: 32
        }
    );
    g.set_viewport(16, 16, 24);
    assert_eq!(
        g.draws(0).next().unwrap().dest,
        PixelRect {
            x: 0,
            y: 0,
            width: 64,
            height: 64
        }
    );
    let outcome = g.command(b"Ga=p,i=1,c=2", context(), &mut Vec::new());
    assert_eq!(outcome.cursor_move, Some((2, 1)));
}

#[test]
fn replacement_and_placement_ids_follow_lifecycle() {
    let mut g = Graphics::default();
    send(
        &mut g,
        b"Ga=T,i=3,p=5,s=1,v=1,C=1",
        &[1, 2, 3, 4],
        context(),
    );
    let before = g.image(3).unwrap().generation;
    command(&mut g, b"Ga=p,i=3,p=5,c=2,C=1");
    assert_eq!(g.placement_count(), 1);
    command(&mut g, b"Ga=p,i=3,C=1");
    assert_eq!(g.placement_count(), 2);
    command(&mut g, b"Ga=d,d=i,i=3,p=5");
    assert_eq!(g.placement_count(), 1);
    send(&mut g, b"Ga=t,i=3,s=1,v=1", &[5, 6, 7, 8], context());
    assert_eq!(g.placement_count(), 0);
    assert!(g.image(3).unwrap().generation > before);
    command(&mut g, b"Ga=p,i=3,C=1");
    command(&mut g, b"Ga=d,d=I,i=3");
    assert_eq!(
        (g.image_count(), g.placement_count(), g.image_bytes()),
        (0, 0, 0)
    );
}

#[test]
fn numbers_allocate_unique_ids_and_delete_newest_only() {
    let mut g = Graphics::default();
    send(&mut g, b"Ga=t,i=1,s=1,v=1", &[1, 2, 3, 4], context());
    assert_eq!(
        send(&mut g, b"Ga=t,I=9,s=1,v=1", &[1, 2, 3, 4], context()),
        b"\x1b_Gi=2,I=9;OK\x1b\\"
    );
    assert_eq!(
        send(&mut g, b"Ga=t,I=9,s=1,v=1", &[1, 2, 3, 4], context()),
        b"\x1b_Gi=3,I=9;OK\x1b\\"
    );
    command(&mut g, b"Ga=d,d=N,I=9");
    assert!(g.image(3).is_none());
    assert!(g.image(2).is_some());
}

#[test]
fn deletion_frees_only_selected_unreferenced_images() {
    let mut g = Graphics::default();
    send(
        &mut g,
        b"Ga=T,i=1,s=1,v=1,z=2,C=1",
        &[1, 2, 3, 4],
        context(),
    );
    send(&mut g, b"Ga=t,i=2,s=1,v=1", &[1, 2, 3, 4], context());
    command(&mut g, b"Ga=d,d=Z,z=2");
    assert!(g.image(1).is_none());
    assert!(g.image(2).is_some());
}

#[test]
fn scroll_keeps_images_in_history_and_trims_expired_placements() {
    let mut g = Graphics::default();
    send(
        &mut g,
        b"Ga=T,i=1,s=1,v=1,r=2,C=1",
        &[1, 2, 3, 4],
        context(),
    );
    g.scroll(0, 23, -1, 1, true);
    assert_eq!(g.draws(0).next().unwrap().dest.y, -16);
    assert_eq!(g.draws(1).next().unwrap().dest.y, 0);
    g.scroll(0, 23, -2, 3, true);
    assert!(g.draws(0).next().is_none());
    assert!(g.draws(3).next().is_some());
    g.trim_history(0);
    assert_eq!(g.placement_count(), 0);
}

#[test]
fn partial_scroll_does_not_move_unaffected_placements() {
    let mut g = Graphics::default();
    for row in [0, 4, 7, 10] {
        send(
            &mut g,
            format!("Ga=T,i={},s=1,v=1,C=1", row + 1).as_bytes(),
            &[1, 2, 3, 4],
            Context { row, ..context() },
        );
    }
    g.scroll(4, 7, -1, 0, false);
    let ys: Vec<_> = g.draws(0).map(|d| (d.image_id, d.dest.y)).collect();
    assert_eq!(ys, [(1, 0), (8, 96), (11, 160)]);
}

#[test]
fn limits_bound_transfer_store_and_placement_counts() {
    let mut g = Graphics::new(Limits {
        max_image_bytes: 8,
        max_total_bytes: 8,
        max_transfer_bytes: 8,
        max_images: 2,
        max_placements: 1,
        ..Limits::default()
    });
    send(&mut g, b"Ga=T,i=1,s=1,v=1,C=1", &[1, 2, 3, 4], context());
    send(&mut g, b"Ga=t,i=2,s=1,v=1", &[1, 2, 3, 4], context());
    send(&mut g, b"Ga=t,i=3,s=1,v=1", &[1, 2, 3, 4], context());
    assert!(g.image(2).is_none());
    assert!(g.image(1).is_some());
    assert_eq!(g.image_bytes(), 8);
    assert!(
        command(&mut g, b"Ga=p,i=3,C=1")
            .windows(6)
            .any(|w| w == b"ENOSPC")
    );
    command(&mut g, b"Ga=t,i=4,s=2,v=1,m=1;AAAA");
    command(&mut g, b"Gm=1;AAAA");
    assert!(
        command(&mut g, b"Gm=1;AAAA")
            .windows(5)
            .any(|w| w == b"E2BIG")
    );
    assert_eq!(g.pending_bytes(), 0);
}

#[test]
fn feature_query_is_honest_and_does_not_cancel_pending_transfer() {
    let mut g = Graphics::default();
    command(&mut g, b"Ga=t,i=4,s=2,v=1,m=1;AAAA");
    assert_eq!(
        command(&mut g, b"Ga=q,i=1,e=T,U=1,C=1,t=f"),
        b"\x1b_Gi=1,e=T,C=1;OK\x1b\\"
    );
    assert_eq!(g.pending_bytes(), 3);
    assert_eq!(command(&mut g, b"Ga=q,i=2,e=f"), b"\x1b_Gi=2;OK\x1b\\");
    assert_eq!(g.pending_bytes(), 3);
}

#[test]
fn quiet_modes_and_response_queue_are_bounded() {
    let mut g = Graphics::default();
    assert!(send(&mut g, b"Ga=q,i=1,f=24,s=1,v=1,q=1", &[1, 2, 3], context()).is_empty());
    assert!(!send(&mut g, b"Ga=q,i=1,f=24,s=1,v=1,q=1", &[1], context()).is_empty());
    assert!(send(&mut g, b"Ga=q,i=1,f=24,s=1,v=1,q=2", &[1], context()).is_empty());
    let mut replies = vec![b'x'; 65530];
    g.command(b"Ga=p,i=999", context(), &mut replies);
    assert_eq!(replies.len(), 65530);
}

#[test]
fn malformed_base64_is_rejected_without_partial_append() {
    for bytes in [
        b"A===".as_slice(),
        b"AA=A",
        b"AB==",
        b"AAB=",
        b"====",
        b"A",
        b"AA\x80A",
    ] {
        let mut out = vec![1, 2, 3];
        assert!(decode::base64_append(bytes, &mut out, 16).is_err());
        assert_eq!(out, [1, 2, 3]);
    }
}

#[test]
fn malformed_png_headers_with_valid_checksums_remain_recoverable() {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in bytes {
            crc ^= byte as u32;
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb88320u32.wrapping_mul(crc & 1));
            }
        }
        !crc
    }
    let original = include_bytes!("../tests/data/rgba.png");
    let mut random = 0x8712ac94u32;
    for _ in 0..512 {
        let mut png = original.to_vec();
        for index in [19, 23, 24, 25, 26, 27, 28] {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            png[index] = (random % 17) as u8;
        }
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        let mut g = Graphics::default();
        send(&mut g, b"Ga=T,i=7,f=100,C=1", &png, context());
        // Recovery is observed through a successful subsequent normal command,
        // not merely through the absence of a panic in the malformed decoder.
        assert_eq!(
            send(&mut g, b"Ga=q,i=8,f=24,s=1,v=1", &[0, 0, 0], context()),
            b"\x1b_Gi=8;OK\x1b\\"
        );
    }
}

#[test]
fn failed_combined_transmission_does_not_evict_or_replace_valid_images() {
    let mut g = Graphics::new(Limits {
        max_total_bytes: 4,
        ..Limits::default()
    });
    send(&mut g, b"Ga=t,i=1,s=1,v=1", &[1, 2, 3, 4], context());
    let generation = g.image(1).unwrap().generation;
    let reply = send(&mut g, b"Ga=T,i=2,s=1,v=1,x=2", &[1, 2, 3, 4], context());
    assert!(reply.windows(6).any(|w| w == b"EINVAL"));
    assert_eq!(g.image(1).unwrap().generation, generation);
    assert_eq!(g.image_count(), 1);
}

#[test]
fn retained_pixels_outlive_scene_deletion_without_copying() {
    let mut g = Graphics::default();
    send(&mut g, b"Ga=T,i=1,s=1,v=1,C=1", &[1, 2, 3, 4], context());
    let retained = g.image(1).unwrap().pixels.clone();
    let pointer = retained.as_ptr();
    command(&mut g, b"Ga=d,d=I,i=1");
    assert_eq!(g.image_bytes(), 0);
    assert_eq!(retained.as_slice(), [1, 2, 3, 4]);
    assert_eq!(retained.as_ptr(), pointer);
}

#[test]
fn padded_and_unpadded_final_base64_decode_identically() {
    for count in 0..100 {
        let bytes: Vec<_> = (0..count).map(|i| (i * 73) as u8).collect();
        let padded = base64(&bytes);
        let mut unpadded = padded.clone();
        while unpadded.last() == Some(&b'=') {
            unpadded.pop();
        }
        for encoded in [&padded, &unpadded] {
            let mut decoded = Vec::new();
            decode::base64_append(encoded, &mut decoded, 1000).unwrap();
            assert_eq!(decoded, bytes);
        }
    }
}

#[test]
fn large_single_apc_like_icat_streams_with_bounded_staging_and_unpadded_tail() {
    let data = include_bytes!("../tests/data/large-inline-rgb.zlib");
    let mut body = b"Ga=T,q=2,f=24,o=z,C=1,s=128,v=128;".to_vec();
    body.extend(base64(data));
    while body.last() == Some(&b'=') {
        body.pop();
    }
    assert!(body.len() > 51_000);
    for chunk_size in [1, 17, 4096, 65536] {
        let mut g = Graphics::default();
        let mut stream = CommandStream::new();
        let mut replies = Vec::new();
        for bytes in body.chunks(chunk_size) {
            stream.push(&mut g, bytes, context(), &mut replies);
            assert!(stream.buffered_bytes() <= 4608);
            assert_eq!(g.image_count(), 0, "no image is published before ST");
        }
        stream.finish(
            &mut g,
            Context {
                col: 3,
                row: 4,
                ..context()
            },
            &mut replies,
        );
        assert!(replies.is_empty());
        let draw = g.draws(0).next().unwrap();
        assert_eq!(
            (
                draw.dest.x,
                draw.dest.y,
                draw.image.width,
                draw.image.height
            ),
            (24, 64, 128, 128)
        );
        for (index, rgba) in draw.image.pixels.as_chunks::<4>().0.iter().enumerate() {
            assert_eq!(
                *rgba,
                [
                    (index * 3 % 251) as u8,
                    ((index * 3 + 1) % 251) as u8,
                    ((index * 3 + 2) % 251) as u8,
                    255
                ]
            );
        }
        assert_eq!(g.pending_bytes(), 0);
    }
}

#[test]
fn streamed_m_flags_quiet_errors_and_transfer_limit_are_preserved() {
    let mut g = Graphics::default();
    let mut stream = CommandStream::new();
    let mut replies = Vec::new();
    stream.push(
        &mut g,
        b"Ga=T,i=9,s=2,v=1,m=1,C=1;/wAA",
        context(),
        &mut replies,
    );
    stream.finish(&mut g, context(), &mut replies);
    assert_eq!(g.pending_bytes(), 3);
    assert_eq!(g.image_count(), 0);
    stream.push(&mut g, b"Gm=0;/wD/AIA", context(), &mut replies);
    stream.finish(&mut g, context(), &mut replies);
    assert_eq!(
        g.image(9).unwrap().pixels.as_slice(),
        [255, 0, 0, 255, 0, 255, 0, 128]
    );
    assert_eq!(replies, b"\x1b_Gi=9;OK\x1b\\");

    let mut g = Graphics::new(Limits {
        max_transfer_bytes: 32,
        ..Limits::default()
    });
    replies.clear();
    stream.begin();
    stream.push(&mut g, b"Ga=T,i=1,s=32,v=32,q=1;", context(), &mut replies);
    for _ in 0..20 {
        stream.push(&mut g, &[b'A'; 4096], context(), &mut replies);
        assert!(g.pending_bytes() <= 32);
    }
    stream.finish(&mut g, context(), &mut replies);
    assert!(replies.windows(5).any(|w| w == b"E2BIG"));
    assert_eq!(g.image_count(), 0);
    stream.push(
        &mut g,
        b"Ga=q,i=2,f=24,s=1,v=1;AAAA",
        context(),
        &mut replies,
    );
    stream.finish(&mut g, context(), &mut replies);
    assert!(replies.ends_with(b"\x1b_Gi=2;OK\x1b\\"));
}

#[test]
fn current_icat_128k_chunks_can_repeat_action_and_quiet_on_continuations() {
    let pixels: Vec<u8> = (0..256 * 256 * 4).map(|i| (i % 251) as u8).collect();
    let mut payload = base64(&pixels);
    while payload.last() == Some(&b'=') {
        payload.pop();
    }
    let count = payload.len().div_ceil(128 * 1024);
    let mut g = Graphics::default();
    let mut stream = CommandStream::new();
    let mut replies = Vec::new();
    for (index, chunk) in payload.chunks(128 * 1024).enumerate() {
        let header: &[u8] = if index == 0 {
            b"Ga=T,q=2,f=32,s=256,v=256,C=1,m=1;".as_slice()
        } else if index + 1 < count {
            b"Ga=T,q=2,m=1;"
        } else {
            b"Ga=T,q=2;"
        };
        stream.push(&mut g, header, context(), &mut replies);
        for bytes in chunk.chunks(65536) {
            stream.push(&mut g, bytes, context(), &mut replies);
        }
        stream.finish(&mut g, context(), &mut replies);
        if index + 1 < count {
            assert_eq!(g.image_count(), 0);
        }
    }
    assert!(replies.is_empty());
    assert_eq!(g.image_count(), 1);
    assert_eq!(g.draws(0).next().unwrap().image.pixels.as_slice(), pixels);
}
