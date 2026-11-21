//! Reproducible wire-size comparison for a single changing status row.
use zterm::Terminal;
use ztty::terminal_frame::{Renderer, capture};

fn main() {
    let mut terminal = Terminal::new(120, 40, 0);
    for row in 1..=40 {
        terminal.feed(
            format!(
                "\x1b[{row};1H\x1b[36m{: <120}\x1b[0m",
                format!("remote task {row}: waiting")
            )
            .as_bytes(),
        );
    }
    let first = capture(&terminal, None).unwrap();
    let mut renderer = Renderer::new();
    let initial = renderer.render(&first).unwrap().len();
    let idle = renderer.render(&first).unwrap().len();
    let mut full = 0usize;
    let mut delta = 0usize;
    for sample in 0..1000 {
        terminal.feed(format!("\x1b[20;1H\x1b[32mprogress {sample:04}\x1b[0m").as_bytes());
        let frame = capture(&terminal, Some(0)).unwrap();
        full += Renderer::new().render(&frame).unwrap().len();
        delta += renderer.render(&frame).unwrap().len();
    }
    println!(
        "{{\"cols\":120,\"rows\":40,\"updates\":1000,\"initial_bytes\":{initial},\"idle_bytes\":{idle},\"full_frame_bytes\":{full},\"delta_bytes\":{delta},\"bytes_saved_percent\":{:.3}}}",
        (1.0 - delta as f64 / full as f64) * 100.0
    );
}
