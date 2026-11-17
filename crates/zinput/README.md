# zinput

Terminal input encoding in Rust. `zinput` converts keyboard, mouse, focus and
clipboard events into the byte sequences consumed by terminal applications.
It is an independent `#![no_std]` crate with zero dependencies and no unsafe
code. It uses `alloc` for returned byte buffers.

The caller supplies the application's active input modes. The library does not
open a window, create a PTY or parse the application's output stream. It can be
used by terminal emulators, embedded terminal frontends and protocol tools.

## Install

```toml
[dependencies]
zinput = "0.1.0"
```

## Encode keyboard input

```rust
use zinput::{encode_key, Key, KeyEvent, Modes, Modifiers};

let legacy = Modes::default();
assert_eq!(
    encode_key(Key::Up, Modifiers(0), KeyEvent::Press, "", &legacy),
    b"\x1b[A",
);

let kitty = Modes { kitty_keyboard: 1, ..Modes::default() };
assert_eq!(
    encode_key(
        Key::Character('c'), Modifiers(Modifiers::CTRL),
        KeyEvent::Press, "\u{3}", &kitty,
    ),
    b"\x1b[99;5u",
);
```

`Key::Character` identifies the **unshifted** key. The `text` argument is the
complete UTF-8 text committed by the platform. Pass `Key::Unidentified` or use
`encode_text` when an input method commits text without a physical key identity.
Multi-character commits are retained.

The modifier bit values are Shift=1, Alt=2, Ctrl=4, Super=8, Hyper=16, Meta=32,
Caps Lock=64 and Num Lock=128. The encoder adds the protocol's modifier offset.
For a modifier key's own event, supply its state after the transition, including
any other physical key that keeps the same modifier active.

## Reuse a buffer

```rust
extern crate alloc;

use alloc::vec::Vec;
use zinput::{encode_key_into, Key, KeyEvent, Modes, Modifiers};

let modes = Modes::default();
let mut bytes = Vec::with_capacity(128);
encode_key_into(Key::Enter, Modifiers(0), KeyEvent::Press, "\r", &modes, &mut bytes);
assert_eq!(bytes, b"\r");

// After the transport accepts these bytes, clear and reuse the same allocation.
bytes.clear();
encode_key_into(Key::Left, Modifiers(0), KeyEvent::Press, "", &modes, &mut bytes);
assert_eq!(bytes, b"\x1b[D");
```

The `_into` functions append to the supplied buffer. They avoid further buffer
allocation while its existing capacity is sufficient. The convenience functions
return a new `Vec<u8>`.

## Protocol coverage

| Input | Behavior |
|---|---|
| Legacy keyboard | UTF-8 text, Control/Alt combinations, application cursor and keypad modes, function keys and modifiers |
| Kitty keyboard | Progressive enhancement flags 1/2/4/8/16, press/repeat/release, alternate keys, modifier keys, keypad identity and associated text |
| Mouse | Tracking modes 1000/1002/1003, SGR 1006 and legacy encoding, modifiers and wheel events |
| Paste | Optional bracketed-paste delimiters; tabs and line endings preserved; other control characters removed |
| Focus | Focus-in/out sequences when focus reporting is enabled |

Kitty alternate-key reporting can infer a shifted character from committed text.
Use `encode_key_with_alternates` with `KeyAlternates` when the platform also knows
the standard-layout key identity. A pure input-method commit uses key 0 with
associated text when flags 8 and 16 are both enabled; otherwise it is emitted as
UTF-8 to preserve input that has no physical key event.

Enter, Tab and Backspace retain their legacy behavior until Kitty flag 8 is
enabled. Function key F3 uses `CSI 13~` in enhanced mode to avoid cursor-report
ambiguity. Modifier-key events require flag 8, and release reports require
flag 2.

Mouse positions are zero-based at the API boundary. The encoder converts them
to one-based protocol coordinates. Buttons are 0=left, 1=middle and 2=right.
SGR encoding alone does not enable tracking. Legacy coordinates beyond its
representable range produce no event. The host should suppress mouse motion
that remains inside the same character cell.

Clipboard encoding removes Escape, DEL and C0/C1 controls other than Tab, CR and
LF. Consequently clipboard contents cannot inject a bracketed-paste end marker.

## Integration contract

The host is responsible for input-mode negotiation, keyboard layout and IME
handling, clipboard access, transport backpressure and delivery order. Report
only modes that are connected to the host's actual event path. The codec owns no
window or operating-system resources and performs no I/O.

## Verification

```sh
cargo test -p zinput
cargo check -p zinput --lib
```

The unit tests cover mode-dependent keyboard behavior, multi-character IME
commits, mouse tracking and coordinate limits, clipboard delimiter injection,
focus reporting and buffer reuse. Test tooling uses Rust's normal test harness;
the published library remains `no_std`.

## Protocol references

- [Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)
- [XTerm control sequences](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)

## License

MIT. Part of [Project Z / ztty](https://github.com/LIghtJUNction/ztty).
