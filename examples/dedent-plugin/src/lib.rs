//! Remove common leading spaces when copying an indented code block.
use ztty_plugin_api::{TextAction, TextActionExport};

struct Dedent;

impl TextAction for Dedent {
    fn transform(&mut self, input: &str, output: &mut [u8]) -> zplugin::Result<u64> {
        let indent = input
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.bytes().take_while(|byte| *byte == b' ').count())
            .min()
            .unwrap_or(0);
        // Output never grows. Check before writing so a short buffer is atomic.
        let required: usize = input
            .split_inclusive('\n')
            .map(|line| {
                line.len()
                    - line
                        .bytes()
                        .take(indent)
                        .take_while(|byte| *byte == b' ')
                        .count()
            })
            .sum();
        if required > output.len() {
            return Err(zplugin::Error::BufferTooSmall);
        }
        let mut written = 0;
        for line in input.split_inclusive('\n') {
            let remove = line
                .bytes()
                .take(indent)
                .take_while(|byte| *byte == b' ')
                .count();
            let bytes = &line.as_bytes()[remove..];
            output[written..written + bytes.len()].copy_from_slice(bytes);
            written += bytes.len();
        }
        Ok(written as u64)
    }
}

fn create() -> zplugin::Result<Dedent> {
    Ok(Dedent)
}

// SAFETY: Dedent owns no threads, tasks, callbacks, buffers or registrations.
// All input/output borrows end with transform; dropping the instance is inert.
unsafe impl zplugin::export::UnloadSafe for Dedent {}

zplugin::export!(
    TextActionExport<Dedent>,
    constructor = create,
    version = (1, 0, 0),
    unload = cooperative
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedent_preserves_relative_indent_unicode_and_line_endings() {
        for (input, expected) in [
            (
                "    fn main() {\n        你好();\n    }\n",
                "fn main() {\n    你好();\n}\n",
            ),
            ("  a\r\n    e\u{301}\r\n", "a\r\n  e\u{301}\r\n"),
            ("  a\n \n  b", "a\n\nb"),
            ("\tcode\n  other", "\tcode\n  other"),
            ("", ""),
        ] {
            let mut out = vec![0; input.len()];
            let size = Dedent.transform(input, &mut out).unwrap() as usize;
            assert_eq!(&out[..size], expected.as_bytes());
        }
    }

    #[test]
    fn short_output_is_not_partially_written() {
        let mut out = [0x5a; 2];
        assert_eq!(
            Dedent.transform("   large", &mut out),
            Err(zplugin::Error::BufferTooSmall)
        );
        assert_eq!(out, [0x5a; 2]);
    }
}
