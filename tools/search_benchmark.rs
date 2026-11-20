//! Private benchmark crate source copied by benchmark_search.py.
//! Before and after compile against the exact same zterm source and settings.
extern crate alloc;
#[allow(dead_code)]
mod before;
#[allow(dead_code)]
mod after;

use std::{hint::black_box, time::Instant};
use zterm::Terminal;

fn before_find(terminal: &Terminal, query: &str, insensitive: bool) -> before::Result {
    before::find(
        black_box(terminal),
        black_box(query),
        if insensitive { before::Case::Insensitive } else { before::Case::Sensitive },
        None,
        true,
    )
}

fn after_find(terminal: &Terminal, query: &str, insensitive: bool) -> after::Result {
    after::find(
        black_box(terminal),
        black_box(query),
        if insensitive { after::Case::Insensitive } else { after::Case::Sensitive },
        None,
        true,
    )
}

fn verify(terminal: &Terminal, query: &str, insensitive: bool) -> usize {
    let a = before_find(terminal, query, insensitive);
    let b = after_find(terminal, query, insensitive);
    assert_eq!((a.count, a.ordinal), (b.count, b.ordinal));
    assert_eq!(
        a.hit.map(|hit| (hit.row, hit.start, hit.end_row, hit.end)),
        b.hit.map(|hit| (hit.row, hit.start, hit.end_row, hit.end)),
    );
    assert_eq!(
        a.hit.and_then(|hit| before::text(terminal, hit)),
        b.hit.and_then(|hit| after::text(terminal, hit)),
    );
    let ca = if insensitive { before::Case::Insensitive } else { before::Case::Sensitive };
    let cb = if insensitive { after::Case::Insensitive } else { after::Case::Sensitive };
    assert_eq!(
        before::highlights(terminal, query, ca, 0),
        after::highlights(terminal, query, cb, 0),
    );
    a.count
}

fn measure(
    terminal: &Terminal,
    query: &str,
    insensitive: bool,
    candidate: bool,
    iterations: usize,
) -> u128 {
    let started = Instant::now();
    for _ in 0..iterations {
        if candidate {
            black_box(after_find(terminal, query, insensitive));
        } else {
            black_box(before_find(terminal, query, insensitive));
        }
    }
    started.elapsed().as_nanos()
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let lines: usize = args[1].parse().unwrap();
    let samples: usize = args[2].parse().unwrap();
    let iterations: usize = args[3].parse().unwrap();
    assert!(lines > 0 && samples > 0 && iterations > 0);
    let digits = "0123456789".repeat(7);
    let repetitive = "ab".repeat(35);
    let unicode_noise = "中Ωe\u{301}".repeat(12);
    let unicode_hit = format!("{}界é{}", "中Ω".repeat(6), "中Ω".repeat(6));
    let case_noise = "ZzΩ中".repeat(12);
    let case_hit = format!("{}Aİ中e\u{301}{}", "ZzΩ".repeat(6), "ZzΩ".repeat(6));
    // The final x is column 78. A following wide character skips column 79,
    // giving a match spanning a soft wrap with wide-glyph padding.
    let wrapped_hit = format!("{}x界é{}", "z".repeat(78), "z".repeat(20));
    let wrapped_noise = "z".repeat(103);
    let sparse_hit = format!("{}needle{}", "0123456789".repeat(3), "0123456789".repeat(3));
    let scenarios = [
        ("no_match_ascii", "needle", false, digits.as_str(), digits.as_str(), 97),
        ("sparse_ascii", "needle", false, digits.as_str(), sparse_hit.as_str(), 97),
        ("repetitive_ascii", "abab", false, repetitive.as_str(), repetitive.as_str(), 1),
        ("utf8_wide_combining", "界é", false, unicode_noise.as_str(), unicode_hit.as_str(), 97),
        ("unicode_lowercase_expansion", "ai\u{307}中e\u{301}", true, case_noise.as_str(), case_hit.as_str(), 97),
        ("soft_wrap_wide_padding", "x界é", false, wrapped_noise.as_str(), wrapped_hit.as_str(), 97),
    ];
    println!("[");
    for (scenario_index, (name, query, insensitive, noise, hit, interval)) in scenarios.iter().enumerate() {
        let mut terminal = Terminal::new(80, 24, lines.saturating_mul(3).saturating_add(64));
        for line in 0..lines {
            terminal.feed(if line % interval == 0 { hit.as_bytes() } else { noise.as_bytes() });
            terminal.feed(b"\r\n");
        }
        let count = verify(&terminal, query, *insensitive);
        // Warm up both implementations; alternate execution order per sample.
        black_box(before_find(&terminal, query, *insensitive));
        black_box(after_find(&terminal, query, *insensitive));
        let mut baseline_ns = Vec::new();
        let mut candidate_ns = Vec::new();
        for sample in 0..samples {
            let mut timings = [0; 2];
            for which in [sample % 2, 1 - sample % 2] {
                timings[which] = measure(&terminal, query, *insensitive, which == 1, iterations);
            }
            baseline_ns.push(timings[0]);
            candidate_ns.push(timings[1]);
        }
        eprintln!("completed {name}: {} physical rows, {count} matches", terminal.history_len() + terminal.rows());
        println!(
            "  {{\"scenario\":\"{name}\",\"logical_lines\":{lines},\"physical_rows\":{},\"columns\":80,\"matches\":{count},\"iterations_per_sample\":{iterations},\"baseline_ns\":{baseline_ns:?},\"candidate_ns\":{candidate_ns:?}}}{}",
            terminal.history_len() + terminal.rows(),
            if scenario_index + 1 == scenarios.len() { "" } else { "," },
        );
    }
    println!("]");
}
