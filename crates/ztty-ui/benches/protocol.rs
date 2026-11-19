use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::Instant;
use ztty_ui::{Frame, MAX_FRAME, PREFIX, State};

// Development-only instrumentation. The production library forbids unsafe code.
// Counts cover the measured single-threaded region, excluding fixture construction.
struct CountingAllocator;
static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static REALLOCS: AtomicU64 = AtomicU64::new(0);
static DEALLOCS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

// SAFETY: Every operation forwards the unchanged pointer/layout contract to System.
// Atomic counters neither retain nor dereference allocated memory.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller supplies a valid allocator layout.
        let result = unsafe { System.alloc(layout) };
        if !result.is_null() && COUNTING.load(Relaxed) {
            ALLOCS.fetch_add(1, Relaxed);
            ALLOCATED_BYTES.fetch_add(layout.size() as u64, Relaxed);
        }
        result
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller supplies a valid allocator layout.
        let result = unsafe { System.alloc_zeroed(layout) };
        if !result.is_null() && COUNTING.load(Relaxed) {
            ALLOCS.fetch_add(1, Relaxed);
            ALLOCATED_BYTES.fetch_add(layout.size() as u64, Relaxed);
        }
        result
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: ptr/layout/size are forwarded without modification to System.
        let result = unsafe { System.realloc(ptr, layout, size) };
        if !result.is_null() && COUNTING.load(Relaxed) {
            REALLOCS.fetch_add(1, Relaxed);
            // Requested size of each successful allocation, not live bytes or RSS.
            ALLOCATED_BYTES.fetch_add(size as u64, Relaxed);
        }
        result
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if COUNTING.load(Relaxed) {
            DEALLOCS.fetch_add(1, Relaxed);
        }
        // SAFETY: ptr and its original layout are forwarded to System.
        unsafe { System.dealloc(ptr, layout) }
    }
}

fn measure(name: &str, count: usize, mut run: impl FnMut(usize)) {
    ALLOCS.store(0, Relaxed);
    REALLOCS.store(0, Relaxed);
    DEALLOCS.store(0, Relaxed);
    ALLOCATED_BYTES.store(0, Relaxed);
    COUNTING.store(true, Relaxed);
    let start = Instant::now();
    for i in 0..count {
        run(i);
    }
    let elapsed = start.elapsed().as_nanos();
    COUNTING.store(false, Relaxed);
    let allocations = ALLOCS.load(Relaxed);
    let reallocations = REALLOCS.load(Relaxed);
    let deallocations = DEALLOCS.load(Relaxed);
    let allocated_bytes = ALLOCATED_BYTES.load(Relaxed);
    println!(
        "{{\"scenario\":\"{name}\",\"allocator_instrumented\":true,\"iterations\":{count},\"total_ns\":{elapsed},\"ns_per_iteration\":{:.2},\"allocations\":{allocations},\"reallocations\":{reallocations},\"deallocations\":{deallocations},\"allocated_bytes\":{allocated_bytes},\"allocations_per_iteration\":{:.4},\"reallocations_per_iteration\":{:.4},\"allocated_bytes_per_iteration\":{:.2}}}",
        elapsed as f64 / count as f64,
        allocations as f64 / count as f64,
        reallocations as f64 / count as f64,
        allocated_bytes as f64 / count as f64
    );
}

fn main() {
    let mut nodes = format!(
        r#"{{"id":1,"type":"column","children":[{}]}}"#,
        (2u32..=256)
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    for id in 2..=256 {
        nodes.push_str(&format!(
            r#",{{"id":{id},"type":"progress","value":0,"label":"Build {id}"}}"#
        ));
    }
    let replace =
        format!(r#"{{"v":1,"op":"replace","surface":1,"rev":1,"root":1,"nodes":[{nodes}]}}"#)
            .into_bytes();
    measure("create_drop_256_nodes", 2_000, |_| {
        let mut state = State::new();
        black_box(state.apply(black_box(&replace)).unwrap());
        black_box(state.scene());
    });
    let patches=(2..=20_001).map(|rev| format!(r#"{{"v":1,"op":"update","surface":1,"base_rev":{},"rev":{rev},"set":[{{"id":128,"value":{}}}]}}"#,rev-1,rev%10001).into_bytes()).collect::<Vec<_>>();
    let mut state = State::new();
    state.apply(&replace).unwrap();
    measure("update_one_of_256_nodes", patches.len(), |i| {
        black_box(state.apply(black_box(&patches[i])).unwrap());
    });
    measure("capabilities_parse", 100_000, |_| {
        black_box(
            state
                .apply(black_box(br#"{"v":1,"op":"capabilities"}"#))
                .unwrap(),
        );
    });
    let too_large = vec![b' '; MAX_FRAME + 1];
    measure("reject_65537_bytes", 100_000, |_| {
        black_box(state.apply(black_box(&too_large)).unwrap_err());
    });
    let image=br#"{"v":1,"op":"replace","surface":1,"rev":1,"root":1,"nodes":[{"id":1,"type":"image","image_id":42,"generation":19,"alt":"Preview"}]}"#;
    measure("create_drop_image_reference", 20_000, |_| {
        let mut state = State::new();
        black_box(state.apply(black_box(image)).unwrap());
    });
    let mut frame_bytes = PREFIX.to_vec();
    frame_bytes.extend_from_slice(image);
    let mut frame = Frame::new();
    measure("collect_image_frame_one_byte_chunks", 10_000, |_| {
        frame.begin();
        for byte in &frame_bytes {
            frame.push(black_box(std::slice::from_ref(byte)));
        }
        black_box(frame.payload().unwrap());
    });
}
