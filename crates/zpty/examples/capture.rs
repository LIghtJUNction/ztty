//! The library is no_std; this standalone demonstration uses std for printing.
use std::ffi::{CStr, CString};
use std::time::{Duration, Instant};
use zpty::{Pty, Size};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let script = CString::new("read line; printf 'reply:%s\\n' \"$line\"; stty size; test -t 0")?;
    let arguments = [c"sh", c"-c", script.as_c_str()];
    let environment: [&CStr; 2] = [c"PATH=/usr/bin:/bin", c"TERM=xterm-256color"];
    // This example has not created any additional threads, and its only C
    // runtime activity before forkpty is the normal single-threaded startup.
    let mut pty =
        unsafe { Pty::spawn(c"/bin/sh", &arguments, &environment, Size::new(80, 24), &[]) }?;
    pty.resize(Size::new(100, 30))?;
    let mut unrelated = std::process::Command::new("/bin/sh")
        .args(["-c", "exit 7"])
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut input = b"hello from a PTY\n".as_slice();
    while !input.is_empty() {
        match pty.write(input) {
            Ok(0) => return Err("PTY accepted no input".into()),
            Ok(n) => input = &input[n..],
            Err(error) if error.is_would_block() => std::thread::sleep(Duration::from_millis(1)),
            Err(error) => return Err(error.into()),
        }
        if Instant::now() > deadline {
            return Err("PTY write timed out".into());
        }
    }
    let mut output = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        match pty.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => output.extend_from_slice(&buffer[..n]),
            Err(error) if error.is_would_block() => std::thread::sleep(Duration::from_millis(1)),
            Err(error) => return Err(error.into()),
        }
        if Instant::now() > deadline {
            return Err("PTY read timed out".into());
        }
    }
    let status = pty.wait_timeout(1000)?.ok_or("child did not exit")?;
    assert!(status.success());
    assert_eq!(unrelated.wait()?.code(), Some(7));
    assert!(String::from_utf8_lossy(&output).contains("reply:hello from a PTY"));
    assert!(String::from_utf8_lossy(&output).contains("30 100"));
    print!("{}", String::from_utf8_lossy(&output));
    Ok(())
}
