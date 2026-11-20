//! A/B harness compiled by benchmark_search_navigation.py with one zterm graph.
extern crate alloc;
#[allow(dead_code)]
mod after;
#[allow(dead_code)]
mod before;

use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    mem::size_of,
    sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering},
    time::Instant,
};
use zterm::Terminal;

struct CountedAllocator;
static COUNT_ALLOCATIONS: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static REALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static DEALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static LIVE_BYTES: AtomicIsize = AtomicIsize::new(0);

#[global_allocator]
static ALLOCATOR: CountedAllocator = CountedAllocator;

unsafe impl GlobalAlloc for CountedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() && COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            LIVE_BYTES.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() && COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            LIVE_BYTES.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            LIVE_BYTES.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        }
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let resized = unsafe { System.realloc(pointer, layout, size) };
        if !resized.is_null() && COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            REALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            LIVE_BYTES.fetch_add(size as isize - layout.size() as isize, Ordering::Relaxed);
        }
        resized
    }
}

struct Scenario {
    name: &'static str,
    query: &'static str,
    insensitive: bool,
    columns: usize,
    noise: String,
    hit: String,
    interval: usize,
}

#[derive(Clone, Copy)]
enum Direction {
    Forward,
    Backward,
    Alternating,
}

impl Direction {
    fn name(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Backward => "backward",
            Self::Alternating => "alternating",
        }
    }

    fn forward(self, step: usize) -> bool {
        match self {
            Self::Forward => true,
            Self::Backward => false,
            Self::Alternating => step % 2 == 0,
        }
    }

    fn prepare_forward(self) -> bool {
        !matches!(self, Self::Backward)
    }
}

#[derive(Clone, Copy)]
enum Phase {
    First,
    Cold,
    Warm,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::First => "first_search",
            Self::Cold => "cold_navigation",
            Self::Warm => "warm_navigation",
        }
    }
}

fn before_case(scenario: &Scenario) -> before::Case {
    if scenario.insensitive {
        before::Case::Insensitive
    } else {
        before::Case::Sensitive
    }
}

fn after_case(scenario: &Scenario) -> after::Case {
    if scenario.insensitive {
        after::Case::Insensitive
    } else {
        after::Case::Sensitive
    }
}

fn old_find(
    terminal: &Terminal,
    scenario: &Scenario,
    from: Option<before::Hit>,
    forward: bool,
) -> before::Result {
    before::find(
        black_box(terminal),
        black_box(scenario.query),
        before_case(scenario),
        black_box(from),
        black_box(forward),
    )
}

fn new_find(
    cache: &mut after::Cache,
    terminal: &Terminal,
    scenario: &Scenario,
    from: Option<after::Hit>,
    forward: bool,
) -> after::Result {
    cache.find(
        black_box(terminal),
        black_box(scenario.query),
        after_case(scenario),
        black_box(from),
        black_box(forward),
    )
}

fn equal(terminal: &Terminal, old: &before::Result, new: &after::Result) {
    assert_eq!((old.count, old.ordinal), (new.count, new.ordinal));
    assert_eq!(
        old.hit
            .map(|hit| (hit.row, hit.start, hit.end_row, hit.end)),
        new.hit
            .map(|hit| (hit.row, hit.start, hit.end_row, hit.end)),
    );
    assert_eq!(
        old.hit.and_then(|hit| before::text(terminal, hit)),
        new.hit.and_then(|hit| after::text(terminal, hit)),
    );
}

fn verify_sequence(
    terminal: &Terminal,
    scenario: &Scenario,
    direction: Direction,
    warm: bool,
    steps: usize,
) {
    let mut cache = Box::<after::Cache>::default();
    let mut old = old_find(terminal, scenario, None, false);
    let mut new = new_find(&mut cache, terminal, scenario, None, false);
    equal(terminal, &old, &new);
    if warm {
        for _ in 0..2 {
            old = old_find(terminal, scenario, old.hit, direction.prepare_forward());
            new = new_find(
                &mut cache,
                terminal,
                scenario,
                new.hit,
                direction.prepare_forward(),
            );
            equal(terminal, &old, &new);
        }
    }
    for step in 0..steps {
        old = old_find(terminal, scenario, old.hit, direction.forward(step));
        new = new_find(
            &mut cache,
            terminal,
            scenario,
            new.hit,
            direction.forward(step),
        );
        equal(terminal, &old, &new);
    }
}

fn verify_wraps(terminal: &Terminal, scenario: &Scenario) -> usize {
    let mut cache = Box::<after::Cache>::default();
    let mut old = old_find(terminal, scenario, None, true);
    let mut new = new_find(&mut cache, terminal, scenario, None, true);
    equal(terminal, &old, &new);
    for direction in [false, true, false, true] {
        old = old_find(terminal, scenario, old.hit, direction);
        new = new_find(&mut cache, terminal, scenario, new.hit, direction);
        equal(terminal, &old, &new);
    }
    for direction in [false, true] {
        old = old_find(terminal, scenario, None, direction);
        new = new_find(&mut cache, terminal, scenario, None, direction);
        equal(terminal, &old, &new);
    }
    old.count
}

fn measure_before(
    terminal: &Terminal,
    scenario: &Scenario,
    phase: Phase,
    direction: Direction,
    steps: usize,
    first_iterations: usize,
) -> u128 {
    if matches!(phase, Phase::First) {
        let started = Instant::now();
        for _ in 0..first_iterations {
            black_box(old_find(terminal, scenario, None, false));
        }
        return started.elapsed().as_nanos();
    }
    let mut started = Instant::now();
    let mut result = old_find(terminal, scenario, None, false);
    if matches!(phase, Phase::Warm) {
        for _ in 0..2 {
            result = old_find(terminal, scenario, result.hit, direction.prepare_forward());
            black_box(&result);
        }
        started = Instant::now();
    }
    for step in 0..steps {
        result = old_find(terminal, scenario, result.hit, direction.forward(step));
        black_box(&result);
    }
    started.elapsed().as_nanos()
}

fn measure_after(
    terminal: &Terminal,
    scenario: &Scenario,
    phase: Phase,
    direction: Direction,
    steps: usize,
    first_iterations: usize,
) -> u128 {
    if matches!(phase, Phase::First) {
        let started = Instant::now();
        for _ in 0..first_iterations {
            let mut cache = Box::<after::Cache>::default();
            black_box(new_find(&mut cache, terminal, scenario, None, false));
        }
        return started.elapsed().as_nanos();
    }
    let mut started = Instant::now();
    let mut cache = Box::<after::Cache>::default();
    let mut result = new_find(&mut cache, terminal, scenario, None, false);
    if matches!(phase, Phase::Warm) {
        for _ in 0..2 {
            result = new_find(
                &mut cache,
                terminal,
                scenario,
                result.hit,
                direction.prepare_forward(),
            );
            black_box(&result);
        }
        started = Instant::now();
    }
    for step in 0..steps {
        result = new_find(
            &mut cache,
            terminal,
            scenario,
            result.hit,
            direction.forward(step),
        );
        black_box(&result);
    }
    started.elapsed().as_nanos()
}

struct Allocation {
    initial_heap: isize,
    retained_heap: isize,
    warm_allocations: usize,
    warm_reallocations: usize,
    warm_deallocations: usize,
    after_drop: isize,
}

fn allocation(terminal: &Terminal, scenario: &Scenario) -> Allocation {
    ALLOCATIONS.store(0, Ordering::Relaxed);
    REALLOCATIONS.store(0, Ordering::Relaxed);
    DEALLOCATIONS.store(0, Ordering::Relaxed);
    LIVE_BYTES.store(0, Ordering::Relaxed);
    assert!(!COUNT_ALLOCATIONS.swap(true, Ordering::Relaxed));
    let mut cache = Box::<after::Cache>::default();
    let first = new_find(&mut cache, terminal, scenario, None, false);
    let initial_heap = LIVE_BYTES.load(Ordering::Relaxed);
    let mut result = new_find(&mut cache, terminal, scenario, first.hit, false);
    let retained_heap = LIVE_BYTES.load(Ordering::Relaxed);
    ALLOCATIONS.store(0, Ordering::Relaxed);
    REALLOCATIONS.store(0, Ordering::Relaxed);
    DEALLOCATIONS.store(0, Ordering::Relaxed);
    // The refill above retains the last two hits. These alternate inside that
    // window; refill allocations in longer timed sequences remain included.
    for step in 0..256 {
        result = new_find(&mut cache, terminal, scenario, result.hit, step % 2 == 0);
        black_box(&result);
    }
    let warm_allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let warm_reallocations = REALLOCATIONS.load(Ordering::Relaxed);
    let warm_deallocations = DEALLOCATIONS.load(Ordering::Relaxed);
    drop(cache);
    let after_drop = LIVE_BYTES.load(Ordering::Relaxed);
    COUNT_ALLOCATIONS.store(false, Ordering::Relaxed);
    assert_eq!(after_drop, 0, "cache allocations must be released on drop");
    assert_eq!(
        (warm_allocations, warm_reallocations, warm_deallocations),
        (0, 0, 0)
    );
    assert!(retained_heap >= 0);
    assert!(
        retained_heap as usize <= size_of::<after::Cache>() + 1024 + 128 * size_of::<after::Hit>()
    );
    Allocation {
        initial_heap,
        retained_heap,
        warm_allocations,
        warm_reallocations,
        warm_deallocations,
        after_drop,
    }
}

struct Timing {
    phase: Phase,
    direction: Direction,
    operations: usize,
    before_ns: Vec<u128>,
    after_ns: Vec<u128>,
}

fn main() {
    let arguments: Vec<_> = std::env::args().collect();
    let lines: usize = arguments[1].parse().unwrap();
    let samples: usize = arguments[2].parse().unwrap();
    let steps: usize = arguments[3].parse().unwrap();
    let first_iterations: usize = arguments[4].parse().unwrap();
    assert!(lines > 0 && samples > 0 && steps > 0 && first_iterations > 0);
    let scenarios = [
        Scenario {
            name: "sparse_ascii",
            query: "needle",
            insensitive: false,
            columns: 80,
            noise: "0123456789".repeat(7),
            hit: format!("{}needle{}", "x".repeat(30), "x".repeat(30)),
            interval: 73,
        },
        Scenario {
            name: "dense_ascii",
            query: "abab",
            insensitive: false,
            columns: 80,
            noise: "ab".repeat(40),
            hit: "ab".repeat(40),
            interval: 1,
        },
        Scenario {
            name: "unicode_lowercase_soft_wrap",
            query: "ai\u{307}中e\u{301}",
            insensitive: true,
            columns: 40,
            noise: "ZzΩ中".repeat(10),
            hit: format!("{}Aİ中e\u{301}\u{301}{}", "z".repeat(38), "z".repeat(10)),
            interval: 7,
        },
        Scenario {
            name: "no_match_ascii",
            query: "needle",
            insensitive: false,
            columns: 80,
            noise: "0123456789".repeat(7),
            hit: "0123456789".repeat(7),
            interval: 1,
        },
    ];
    println!("[");
    for (scenario_index, scenario) in scenarios.iter().enumerate() {
        let mut terminal = Terminal::new(
            scenario.columns,
            24,
            lines.saturating_mul(3).saturating_add(64),
        );
        for line in 0..lines {
            terminal.feed(if line % scenario.interval == 0 {
                scenario.hit.as_bytes()
            } else {
                scenario.noise.as_bytes()
            });
            terminal.feed(b"\r\n");
        }
        let matches = verify_wraps(&terminal, scenario);
        for direction in [
            Direction::Forward,
            Direction::Backward,
            Direction::Alternating,
        ] {
            for warm in [false, true] {
                verify_sequence(&terminal, scenario, direction, warm, steps);
            }
        }
        eprintln!(
            "verified {}: {} rows, {matches} matches, {steps} steps per navigation sequence",
            scenario.name,
            terminal.history_len() + terminal.rows()
        );
        let mut timings = Vec::new();
        for phase in [Phase::First, Phase::Cold, Phase::Warm] {
            let directions: &[Direction] = if matches!(phase, Phase::First) {
                &[Direction::Backward]
            } else {
                &[
                    Direction::Forward,
                    Direction::Backward,
                    Direction::Alternating,
                ]
            };
            for &direction in directions {
                black_box(measure_before(
                    &terminal,
                    scenario,
                    phase,
                    direction,
                    steps.min(16),
                    1,
                ));
                black_box(measure_after(
                    &terminal,
                    scenario,
                    phase,
                    direction,
                    steps.min(16),
                    1,
                ));
                let mut before_ns = Vec::with_capacity(samples);
                let mut after_ns = Vec::with_capacity(samples);
                for sample in 0..samples {
                    let mut pair = [0; 2];
                    for implementation in [sample % 2, 1 - sample % 2] {
                        pair[implementation] = if implementation == 0 {
                            measure_before(
                                &terminal,
                                scenario,
                                phase,
                                direction,
                                steps,
                                first_iterations,
                            )
                        } else {
                            measure_after(
                                &terminal,
                                scenario,
                                phase,
                                direction,
                                steps,
                                first_iterations,
                            )
                        };
                    }
                    before_ns.push(pair[0]);
                    after_ns.push(pair[1]);
                }
                let operations = match phase {
                    Phase::First => first_iterations,
                    Phase::Cold => steps + 1,
                    Phase::Warm => steps,
                };
                timings.push(Timing {
                    phase,
                    direction,
                    operations,
                    before_ns,
                    after_ns,
                });
            }
        }
        let memory = allocation(&terminal, scenario);
        println!(
            "{{\"scenario\":\"{}\",\"verified\":true,\"logical_lines\":{lines},\"physical_rows\":{},\"columns\":{},\"matches\":{matches},\"navigation_steps\":{steps},\"hit_size_bytes\":{},\"cache_header_bytes\":{},",
            scenario.name,
            terminal.history_len() + terminal.rows(),
            scenario.columns,
            size_of::<after::Hit>(),
            size_of::<after::Cache>()
        );
        println!(
            "\"allocation\":{{\"initial_heap_bytes\":{},\"retained_heap_bytes\":{},\"warm_navigation_steps\":256,\"warm_allocations\":{},\"warm_reallocations\":{},\"warm_deallocations\":{},\"heap_bytes_after_drop\":{}}},",
            memory.initial_heap,
            memory.retained_heap,
            memory.warm_allocations,
            memory.warm_reallocations,
            memory.warm_deallocations,
            memory.after_drop
        );
        println!("\"timings\":[");
        for (index, timing) in timings.iter().enumerate() {
            println!(
                "{{\"phase\":\"{}\",\"direction\":\"{}\",\"operations_per_sample\":{},\"baseline_ns\":{:?},\"candidate_ns\":{:?}}}{}",
                timing.phase.name(),
                timing.direction.name(),
                timing.operations,
                timing.before_ns,
                timing.after_ns,
                if index + 1 == timings.len() { "" } else { "," }
            );
        }
        println!(
            "]}}{}",
            if scenario_index + 1 == scenarios.len() {
                ""
            } else {
                ","
            }
        );
    }
    println!("]");
}
