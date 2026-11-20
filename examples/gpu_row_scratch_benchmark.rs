//! Compare warmed Rust allocation counts and frame submission time at a fixed
//! revision. Copy this identical harness into the baseline worktree for A/B.
//!
//! LIBGL_ALWAYS_SOFTWARE=1 cargo run --release --example gpu_row_scratch_benchmark -- 100 /tmp/ztty-gpu
//!
//! The optional directory receives raw, top-to-bottom RGBA for exact pixel
//! comparison. Timings exclude readback and run with allocation counting off;
//! they measure CPU/EGL submission, not physical input-to-display latency.
//! Rust allocator counts do not include native font/GL allocations or GPU memory.
#[cfg(target_os = "linux")]
mod benchmark {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        path::PathBuf,
        sync::atomic::{AtomicBool, AtomicU64, Ordering},
        time::Instant,
    };
    use ztty::gpu::{Display, GridStyle, Renderer, Target};

    struct CountingAllocator;

    static COUNTING: AtomicBool = AtomicBool::new(false);
    static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
    static REALLOCATIONS: AtomicU64 = AtomicU64::new(0);
    static DEALLOCATIONS: AtomicU64 = AtomicU64::new(0);
    static REQUESTED_BYTES: AtomicU64 = AtomicU64::new(0);

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;

    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let pointer = unsafe { System.alloc(layout) };
            if !pointer.is_null() && COUNTING.load(Ordering::Relaxed) {
                ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
                REQUESTED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            }
            pointer
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let pointer = unsafe { System.alloc_zeroed(layout) };
            if !pointer.is_null() && COUNTING.load(Ordering::Relaxed) {
                ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
                REQUESTED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            }
            pointer
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            if COUNTING.load(Ordering::Relaxed) {
                DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            }
            unsafe { System.dealloc(pointer, layout) };
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            let result = unsafe { System.realloc(pointer, layout, size) };
            if !result.is_null() && COUNTING.load(Ordering::Relaxed) {
                REALLOCATIONS.fetch_add(1, Ordering::Relaxed);
                REQUESTED_BYTES.fetch_add(size as u64, Ordering::Relaxed);
            }
            result
        }
    }

    pub fn run() {
        let mut arguments = std::env::args().skip(1);
        let frames = arguments
            .next()
            .map(|value| value.parse::<usize>().expect("frame count must be an integer"))
            .unwrap_or(100);
        assert!((1..=5000).contains(&frames), "frame count must be 1..=5000");
        let output = arguments.next().map(PathBuf::from);
        assert!(arguments.next().is_none(), "usage: [frames] [RGBA output directory]");
        if let Some(directory) = &output {
            std::fs::create_dir_all(directory).unwrap();
        }
        let cols = 96;
        let rows = 24;
        let scenarios = [
            (
                "ascii",
                "0123456789 abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            ),
            (
                "unicode",
                "中文 e\u{301} 👩🏽‍💻 office سلام 👨‍👩‍👧‍👦 🇨🇳",
            ),
            (
                "styled",
                "\x1b[1;31mbold\x1b[0m \x1b[3;32mitalic\x1b[0m \x1b[4munderlined\x1b[0m \x1b[9mstrike\x1b[0m 中 👩🏽‍💻",
            ),
        ];
        for (name, line) in scenarios {
            let mut terminal = zterm::Terminal::new(cols, rows, 0);
            for row in 0..rows {
                terminal.feed(format!("\x1b[{};1H", row + 1).as_bytes());
                terminal.feed(line.as_bytes());
            }
            let mut renderer = Renderer::new(
                Display::headless().unwrap(),
                Target::Offscreen,
                1024,
                768,
                "DejaVu Sans Mono:size=14",
            )
            .unwrap();
            let metrics = renderer.metrics();
            let width = (metrics.cell_width * cols as f32).ceil() as u32;
            let height = (metrics.cell_height * rows as f32).ceil() as u32;
            renderer.resize(width, height).unwrap();
            let style = GridStyle {
                padding: 0.0,
                cell_width: metrics.cell_width,
                cell_height: metrics.cell_height,
                ascent: metrics.ascent,
                background: 0x101010,
                background_alpha: 255,
                selection_foreground: 0x000000,
                selection_background: 0xffffff,
                cursor: Some(((3, 2), zterm::CursorShape::Block)),
                cursor_color: 0x55ff55,
                slow_blink_visible: true,
                rapid_blink_visible: true,
            };
            let selected = |x, y| y == 9 && (1..17).contains(&x);
            for _ in 0..5 {
                renderer
                    .draw_terminal(&terminal, 0, style, selected)
                    .unwrap();
                renderer.present().unwrap();
            }
            let reference = renderer.read_pixels().unwrap();

            ALLOCATIONS.store(0, Ordering::Relaxed);
            REALLOCATIONS.store(0, Ordering::Relaxed);
            DEALLOCATIONS.store(0, Ordering::Relaxed);
            REQUESTED_BYTES.store(0, Ordering::Relaxed);
            COUNTING.store(true, Ordering::Relaxed);
            for _ in 0..frames {
                renderer
                    .draw_terminal(&terminal, 0, style, selected)
                    .unwrap();
                renderer.present().unwrap();
            }
            COUNTING.store(false, Ordering::Relaxed);
            let allocations = ALLOCATIONS.load(Ordering::Relaxed);
            let reallocations = REALLOCATIONS.load(Ordering::Relaxed);
            let deallocations = DEALLOCATIONS.load(Ordering::Relaxed);
            let requested_bytes = REQUESTED_BYTES.load(Ordering::Relaxed);
            assert_eq!(renderer.read_pixels().unwrap(), reference);

            let mut elapsed = Vec::with_capacity(frames);
            for _ in 0..frames {
                let begin = Instant::now();
                renderer
                    .draw_terminal(&terminal, 0, style, selected)
                    .unwrap();
                renderer.present().unwrap();
                elapsed.push(begin.elapsed().as_nanos() as u64);
            }
            let pixels = renderer.read_pixels().unwrap();
            assert_eq!(pixels, reference);
            elapsed.sort_unstable();
            let median = elapsed[(frames * 50).div_ceil(100) - 1];
            let p95 = elapsed[(frames * 95).div_ceil(100) - 1];
            let maximum = elapsed[frames - 1];
            // A compact identity for quick comparison, not a cryptographic hash.
            // The optional raw files allow byte-for-byte or SHA-256 verification.
            let checksum = pixels.iter().fold(0xcbf29ce484222325u64, |value, byte| {
                (value ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
            });
            if let Some(directory) = &output {
                std::fs::write(directory.join(format!("{name}.rgba")), &pixels).unwrap();
            }
            eprintln!("scenario={name} driver={}", renderer.driver());
            println!(
                concat!(
                    "{{\"scenario\":\"{}\",\"frames\":{},\"cols\":{},\"rows\":{},",
                    "\"width\":{},\"height\":{},\"rust_allocations\":{},",
                    "\"rust_reallocations\":{},\"rust_deallocations\":{},",
                    "\"rust_requested_bytes\":{},\"submission_p50_ns\":{},",
                    "\"submission_p95_ns\":{},\"submission_max_ns\":{},",
                    "\"rgba_fnv1a64\":\"{:016x}\"}}",
                ),
                name,
                frames,
                cols,
                rows,
                width,
                height,
                allocations,
                reallocations,
                deallocations,
                requested_bytes,
                median,
                p95,
                maximum,
                checksum,
            );
        }
    }
}

#[cfg(target_os = "linux")]
fn main() {
    benchmark::run();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("This benchmark requires the Linux EGL renderer.");
}
