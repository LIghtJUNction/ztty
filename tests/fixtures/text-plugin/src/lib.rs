//! Fault injection only. Never install this library as a user extension.
#[cfg(not(feature = "wrong-interface"))]
use ztty_plugin_api::{TextAction, TextActionExport};

#[cfg(feature = "wrong-interface")]
#[zplugin::interface(id = "dev.project-z.ztty.text", version = 2)]
pub trait TextAction {
    fn transform(&mut self, input: &str, output: &mut [u8]) -> zplugin::Result<u64>;
}

struct Fixture;

fn marker(line: &str) {
    use std::io::Write;
    if let Ok(path) = std::env::var("ZTTY_PLUGIN_TEST_MARKER") {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(file, "{line}").unwrap();
    }
}

fn create() -> zplugin::Result<Fixture> {
    marker(&format!("create {}", std::process::id()));
    Ok(Fixture)
}

impl Drop for Fixture {
    fn drop(&mut self) {
        marker("destroy");
    }
}

impl TextAction for Fixture {
    fn transform(&mut self, input: &str, output: &mut [u8]) -> zplugin::Result<u64> {
        assert!(input.len() <= ztty_plugin_api::MAX_TEXT_BYTES);
        marker("transform");
        let text = match input.trim_end() {
            "invalid-count" => return Ok(u64::MAX),
            "invalid-utf8" => {
                output[0] = 0xff;
                return Ok(1);
            }
            "panic" => panic!("synthetic plugin panic"),
            "crash" => std::process::abort(),
            "timeout" => {
                std::thread::sleep(std::time::Duration::from_secs(30));
                "unexpected"
            }
            "cpu" => loop {
                std::hint::spin_loop();
            },
            "child-timeout" | "leaked-channel" => {
                // Deliberately violate cooperative cleanup to test the host's
                // process-group fallback; this is not an example for authors.
                let child = std::process::Command::new("/bin/sleep")
                    .arg("30")
                    .spawn()
                    .unwrap();
                marker(&format!("child {}", child.id()));
                if input.trim_end() == "child-timeout" {
                    std::thread::sleep(std::time::Duration::from_secs(30));
                }
                "child"
            }
            "stdout-junk" => {
                use std::io::Write;
                std::io::stdout()
                    .write_all(b"not-a-protocol-header")
                    .unwrap();
                std::io::stdout().flush().unwrap();
                "junk"
            }
            "empty" => "",
            "limits" => {
                let limits = std::fs::read_to_string("/proc/self/limits").unwrap();
                let fds = std::fs::read_dir("/proc/self/fd").unwrap().count();
                let text = format!("fds={fds}\n{limits}");
                output[..text.len()].copy_from_slice(text.as_bytes());
                return Ok(text.len() as u64);
            }
            _ => input,
        };
        if text.len() > output.len() {
            return Err(zplugin::Error::BufferTooSmall);
        }
        output[..text.len()].copy_from_slice(text.as_bytes());
        Ok(text.len() as u64)
    }
}

// SAFETY: the ordinary fixture owns no Rust references, background callbacks or
// threads after a call. Child fault modes intentionally break the application
// lifecycle promise to exercise the supervisor; they hold no library pointers.
unsafe impl zplugin::export::UnloadSafe for Fixture {}

#[cfg(not(feature = "pinned"))]
zplugin::export!(
    TextActionExport<Fixture>,
    constructor = create,
    version = (1, 0, 0),
    unload = cooperative
);

#[cfg(feature = "pinned")]
zplugin::export!(
    TextActionExport<Fixture>,
    constructor = create,
    version = (1, 0, 0),
    unload = pinned
);
