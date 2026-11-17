//! The library is no_std; this standalone demonstration uses std for printing.
#[cfg(unix)]
use std::ffi::{CStr, CString};
use std::time::{Duration, Instant};
use zpty::{Pty, Size};

#[cfg(unix)]
fn session() -> Result<(Pty, std::process::Child), Box<dyn std::error::Error>> {
    let script = CString::new("read line; printf 'reply:%s\\n' \"$line\"; stty size; test -t 0")?;
    let arguments = [c"sh", c"-c", script.as_c_str()];
    let environment: [&CStr; 2] = [c"PATH=/usr/bin:/bin", c"TERM=xterm-256color"];
    // This example has not created any additional threads, and its only C
    // runtime activity before forkpty is the normal single-threaded startup.
    let pty = unsafe { Pty::spawn(c"/bin/sh", &arguments, &environment, Size::new(80, 24), &[]) }?;
    pty.resize(Size::new(100, 30))?;
    let unrelated = std::process::Command::new("/bin/sh")
        .args(["-c", "exit 7"])
        .spawn()?;
    Ok((pty, unrelated))
}

#[cfg(windows)]
fn session() -> Result<(Pty, std::process::Child), Box<dyn std::error::Error>> {
    use std::os::windows::ffi::OsStrExt;
    let mut program =
        std::path::PathBuf::from(std::env::var_os("SystemRoot").ok_or("SystemRoot is not set")?);
    program.push(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let executable: Vec<u16> = program.as_os_str().encode_wide().collect();
    let script = "$line=[Console]::ReadLine(); [Console]::WriteLine('reply:'+$line); [Console]::WriteLine([Console]::WindowHeight.ToString()+' '+[Console]::WindowWidth.ToString()); exit 0";
    let options: Vec<Vec<u16>> = [
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ]
    .iter()
    .map(|s| s.encode_utf16().collect())
    .collect();
    let mut arguments = vec![executable.as_slice()];
    arguments.extend(options.iter().map(Vec::as_slice));
    let pty = Pty::spawn_utf16(&executable, &arguments, None, Size::new(80, 24))?;
    pty.resize(Size::new(100, 30))?;
    let unrelated =
        std::process::Command::new(std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into()))
            .args(["/D", "/C", "exit 7"])
            .spawn()?;
    Ok((pty, unrelated))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (mut pty, mut unrelated) = session()?;
    let deadline = Instant::now() + Duration::from_secs(15);
    #[cfg(unix)]
    let mut input = b"hello from a PTY\n".as_slice();
    #[cfg(windows)]
    let mut input = b"hello from a PTY\r\n".as_slice();
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
        // Old inbox ConPTY keeps its pipe alive until ClosePseudoConsole.
        // Once the expected output and owned child status are observed, the
        // example can finish without treating a quiet pipe as premature EOF.
        if pty.reap()?.is_some()
            && String::from_utf8_lossy(&output).contains("reply:hello from a PTY")
            && String::from_utf8_lossy(&output).contains("30 100")
        {
            break;
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
