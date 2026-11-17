//! A dependency-free pixel probe for the independent libsixel comparison.
use std::io::{self, Read, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let chunk: usize = std::env::args()
        .nth(1)
        .map_or(Ok(256), |value| value.parse())?;
    if chunk == 0 || chunk > 65536 {
        return Err("chunk must be 1..65536".into());
    }
    let mut input = Vec::new();
    io::stdin().take(20 * 1024 * 1024).read_to_end(&mut input)?;
    let mut terminal = zterm::Terminal::new(80, 24, 0);
    terminal.feed(b"\x1b[?80h");
    for bytes in input.chunks(chunk) {
        terminal.feed(bytes);
    }
    let draw = terminal
        .graphics_draws(0)
        .next()
        .ok_or("no completed image")?;
    let mut output = io::stdout().lock();
    output.write_all(&draw.image.width.to_le_bytes())?;
    output.write_all(&draw.image.height.to_le_bytes())?;
    output.write_all(&draw.image.pixels)?;
    Ok(())
}
