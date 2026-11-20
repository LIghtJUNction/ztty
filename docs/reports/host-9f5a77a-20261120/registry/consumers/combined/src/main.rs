fn main() -> Result<(), Box<dyn std::error::Error>> {
    ztty_registry_check_zinput_zgraphics_zpty_zgpui_ztty_plugin_api_zterm::exercise();
    use std::time::{Duration, Instant};
    let argv = [c"sh", c"-c", c"printf registry-pty-ok"];
    let env = [c"PATH=/usr/bin:/bin", c"TERM=xterm-256color"];
    // This fixture is single-threaded and starts no callbacks before forkpty.
    let mut pty = unsafe { zpty::Pty::spawn(c"/bin/sh", &argv, &env,
        zpty::Size::new(80, 24), &[]) }?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    let mut buffer = [0; 256];
    loop {
        match pty.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => output.extend_from_slice(&buffer[..n]),
            Err(error) if error.is_would_block() => std::thread::sleep(Duration::from_millis(1)),
            Err(error) => return Err(error.into()),
        }
        if Instant::now() > deadline { return Err("PTY consumer timed out".into()); }
    }
    assert_eq!(output, b"registry-pty-ok");
    assert!(pty.wait_timeout(1000)?.expect("child exited").success());
    println!("registry consumer passed");
    Ok(())
}
