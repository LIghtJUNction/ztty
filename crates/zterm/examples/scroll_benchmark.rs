//! Counted VT scrolling on a retained screen; excludes PTY and rendering.
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;
use zterm::Terminal;

struct Counted;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            ALLOCATIONS.fetch_add(1, Relaxed);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, old: Layout, size: usize) -> *mut u8 {
        let ptr = unsafe { System.realloc(ptr, old, size) };
        if !ptr.is_null() {
            ALLOCATIONS.fetch_add(1, Relaxed);
        }
        ptr
    }
}

#[global_allocator]
static ALLOCATOR: Counted = Counted;

fn main() {
    const ITERATIONS: usize = 1024;
    println!("{{\"schema\":1,\"scope\":\"VT parser and screen; no PTY or rendering\",");
    println!("\"iterations_per_sample\":{ITERATIONS},\"cases\":[");
    let mut first = true;
    for (cols, rows) in [(120, 48), (240, 80)] {
        for count in [1, 8, 24] {
            for command in ['S', 'T'] {
                let mut term = Terminal::new(cols, rows, 0);
                // Rotate the backing ring before selecting an interior region.
                term.feed(&vec![b'x'; cols * (rows + 7)]);
                term.feed(format!("\x1b[2;{}r\x1b[48;2;1;2;3m", rows - 1).as_bytes());
                let input = format!("\x1b[{count}{command}");
                for _ in 0..ITERATIONS {
                    term.feed(input.as_bytes());
                    term.clear_dirty();
                }
                let before = ALLOCATIONS.load(Relaxed);
                let mut samples = [0u128; 7];
                for sample in &mut samples {
                    let start = Instant::now();
                    for _ in 0..ITERATIONS {
                        term.feed(black_box(input.as_bytes()));
                        term.clear_dirty();
                    }
                    *sample = start.elapsed().as_nanos() / ITERATIONS as u128;
                    black_box(term.cell(0, 0));
                }
                let allocations = ALLOCATIONS.load(Relaxed) - before;
                if !first {
                    println!(",");
                }
                first = false;
                print!(
                    "{{\"cols\":{cols},\"rows\":{rows},\"count\":{count},\"command\":\"{command}\",\"ns_per_scroll\":{samples:?},\"allocations\":{allocations}}}"
                );
            }
        }
    }
    println!("\n]}}");
}
