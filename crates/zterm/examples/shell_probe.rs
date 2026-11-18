//! Run captured or live shell PTY bytes through the production terminal model.
use std::fmt::Write as _;
use std::io::{self, Read, Write as _};

fn hex(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);
    for byte in text.bytes() {
        write!(&mut result, "{byte:02x}").unwrap();
    }
    result
}

fn main() {
    let mode = std::env::args().nth(1);
    if mode.as_deref() == Some("--protocol") {
        println!("zterm-shell-probe-v1");
        return;
    }
    assert!(
        mode.is_none() || mode.as_deref() == Some("--live"),
        "unknown probe mode"
    );
    let mut terminal = zterm::Terminal::new(120, 40, 1024);
    if mode.as_deref() == Some("--live") {
        let mut input = io::stdin().lock();
        let mut output = io::stdout().lock();
        let mut bytes = vec![0; 65536];
        loop {
            let mut size = [0; 4];
            input.read_exact(&mut size).unwrap();
            let size = u32::from_le_bytes(size) as usize;
            if size == 0 {
                break;
            }
            assert!(size <= bytes.len(), "PTY chunk exceeds 64 KiB");
            input.read_exact(&mut bytes[..size]).unwrap();
            terminal.feed(&bytes[..size]);
            let responses = terminal.responses();
            output
                .write_all(&(responses.len() as u32).to_le_bytes())
                .unwrap();
            output.write_all(responses).unwrap();
            output.flush().unwrap();
            terminal.clear_responses();
        }
    } else {
        let mut bytes = Vec::new();
        io::stdin()
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .unwrap();
        assert!(bytes.len() <= 1024 * 1024, "transcript exceeds 1 MiB");
        for chunk in bytes.chunks(13) {
            terminal.feed(chunk);
            terminal.clear_responses();
        }
    }
    for command in terminal.commands() {
        if let Some(output) = terminal.command_output(command.id, zterm::MAX_COMMAND_TEXT) {
            println!(
                "command\t{}\t{}\t{}\t{}\t{}",
                command.id,
                command
                    .exit_status
                    .map_or_else(|| "-".to_owned(), |status| status.to_string()),
                output.complete,
                output.truncated,
                hex(&output.text),
            );
        }
    }
}
