//! Reproducible development-only measurements; the shipped terminal is no_std.
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;
use zterm::{Cell, Terminal};

struct Counted;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            ALLOCATIONS.fetch_add(1, Relaxed);
            LIVE.fetch_add(layout.size(), Relaxed);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, old: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(ptr, old, size) };
        if !result.is_null() {
            ALLOCATIONS.fetch_add(1, Relaxed);
            LIVE.fetch_add(size, Relaxed);
            LIVE.fetch_sub(old.size(), Relaxed);
        }
        result
    }
}

#[global_allocator]
static ALLOCATOR: Counted = Counted;

fn throughput(data: &[u8], history: usize) -> (f64, usize) {
    let mut term = Terminal::new(120, 32, history);
    // Fill history and recycle every row before measuring steady state.
    for _ in 0..4096 {
        term.feed(data);
        term.clear_dirty();
    }
    let mut rates = [0.0f64; 7];
    let mut allocations = 0;
    for rate in &mut rates {
        let before = ALLOCATIONS.load(Relaxed);
        let start = Instant::now();
        for _ in 0..4096 {
            term.feed(black_box(data));
            term.clear_dirty();
        }
        let elapsed = start.elapsed().as_secs_f64();
        allocations += ALLOCATIONS.load(Relaxed) - before;
        *rate = (data.len() * 4096) as f64 / elapsed / 1_048_576.0;
        black_box(term.cell(0, 0));
    }
    rates.sort_by(f64::total_cmp);
    (rates[3], allocations)
}

fn latency(data: &[u8]) -> (u128, u128, u128, usize) {
    let mut term = Terminal::new(120, 32, 0);
    let mut samples = vec![0u128; 10_000];
    for _ in 0..1024 {
        term.feed(data);
    }
    let before = ALLOCATIONS.load(Relaxed);
    for sample in &mut samples {
        let start = Instant::now();
        for _ in 0..16 {
            term.feed(black_box(data));
            term.clear_dirty();
        }
        *sample = start.elapsed().as_nanos() / 16;
        black_box(term.cell(0, 0));
    }
    let allocations = ALLOCATIONS.load(Relaxed) - before;
    samples.sort_unstable();
    (samples[5_000], samples[9_500], samples[9_900], allocations)
}

fn main() {
    let ascii = b"The quick brown fox jumps over the lazy dog. 0123456789 terminal throughput.\r\n"
        .repeat(32);
    let sgr = b"\x1b[38;2;99;179;237mProject Z\x1b[0m \x1b[1;32mready\x1b[0m \x1b[48;5;24m background \x1b[0m\r\n".repeat(32);
    let unicode = "终端响应 · Ελληνικά · 日本語 · e\u{301} · 안녕하세요\r\n"
        .as_bytes()
        .repeat(32);
    let burst = [b'x'; 1024];
    let (ascii_mib, ascii_alloc) = throughput(&ascii, 0);
    let (history_mib, history_alloc) = throughput(&ascii, 2048);
    let (sgr_mib, sgr_alloc) = throughput(&sgr, 0);
    let (unicode_mib, unicode_alloc) = throughput(&unicode, 0);
    let edit = latency(b"\x1b[Hq");
    let burst = latency(&burst);
    let before = LIVE.load(Relaxed);
    let term = Terminal::new(120, 32, 2048);
    let initial_heap = LIVE.load(Relaxed) - before;
    black_box(&term);
    println!("{{");
    println!("  \"schema\": 1,");
    println!("  \"scope\": \"parser plus screen model; no PTY, renderer or display\",");
    println!("  \"geometry\": [120, 32],");
    println!("  \"cell_bytes\": {},", std::mem::size_of::<Cell>());
    println!("  \"initial_core_heap_bytes\": {initial_heap},");
    println!("  \"ascii_mib_s\": {ascii_mib:.3},");
    println!("  \"ascii_full_history_mib_s\": {history_mib:.3},");
    println!("  \"sgr_mib_s\": {sgr_mib:.3},");
    println!("  \"unicode_mib_s\": {unicode_mib:.3},");
    println!(
        "  \"steady_state_allocations\": {},",
        ascii_alloc + history_alloc + sgr_alloc + unicode_alloc
    );
    println!(
        "  \"edit_ns\": {{\"p50\": {}, \"p95\": {}, \"p99\": {}, \"allocations\": {}}},",
        edit.0, edit.1, edit.2, edit.3
    );
    println!(
        "  \"burst_1024_ns\": {{\"p50\": {}, \"p95\": {}, \"p99\": {}, \"allocations\": {}}}",
        burst.0, burst.1, burst.2, burst.3
    );
    println!("}}");
}
