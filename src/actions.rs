//! Named view actions and bounded user key/mouse bindings.
use alloc::{format, string::String, vec::Vec};

pub struct Action {
    pub name: &'static str,
    pub label: &'static str,
}

pub const ACTIONS: &[Action] = &[
    Action {
        name: "new-tab",
        label: "New tab",
    },
    Action {
        name: "split-right",
        label: "Split right",
    },
    Action {
        name: "split-down",
        label: "Split below",
    },
    Action {
        name: "next-tab",
        label: "Next tab",
    },
    Action {
        name: "previous-tab",
        label: "Previous tab",
    },
    Action {
        name: "next-pane",
        label: "Next pane",
    },
    Action {
        name: "previous-pane",
        label: "Previous pane",
    },
    Action {
        name: "swap-next",
        label: "Swap with next pane",
    },
    Action {
        name: "swap-previous",
        label: "Swap with previous pane",
    },
    Action {
        name: "maximize",
        label: "Maximize or restore pane",
    },
    Action {
        name: "grow",
        label: "Grow pane",
    },
    Action {
        name: "shrink",
        label: "Shrink pane",
    },
    Action {
        name: "new-window",
        label: "New window",
    },
    Action {
        name: "move-window",
        label: "Move pane to its own window",
    },
    Action {
        name: "detach",
        label: "Detach and keep session running",
    },
    Action {
        name: "close",
        label: "Close this view",
    },
    Action {
        name: "take-control",
        label: "Take local input control",
    },
    Action {
        name: "search",
        label: "Search terminal history",
    },
    Action {
        name: "palette",
        label: "Open command palette",
    },
    Action {
        name: "sessions",
        label: "Switch or attach a session",
    },
    Action {
        name: "copy",
        label: "Copy selection",
    },
    Action {
        name: "paste",
        label: "Paste clipboard",
    },
    Action {
        name: "paste-primary",
        label: "Paste primary selection",
    },
    Action {
        name: "scroll-up",
        label: "Scroll one page up",
    },
    Action {
        name: "scroll-down",
        label: "Scroll one page down",
    },
    Action {
        name: "previous-command",
        label: "Jump to previous shell command",
    },
    Action {
        name: "next-command",
        label: "Jump to next shell command",
    },
    Action {
        name: "copy-command-output",
        label: "Copy selected or latest command output",
    },
    Action {
        name: "reload",
        label: "Reload configuration",
    },
    Action {
        name: "font-increase",
        label: "Increase font size",
    },
    Action {
        name: "font-decrease",
        label: "Decrease font size",
    },
    Action {
        name: "font-reset",
        label: "Reset font size",
    },
    Action {
        name: "none",
        label: "Disable a binding",
    },
];

/// Every whitespace-separated word must occur in at least one field. Matching
/// ignores ASCII case without allocating a lowercase copy of each field.
pub fn matches(query: &str, fields: &[&str]) -> bool {
    query.split_whitespace().all(|word| {
        fields.iter().any(|field| {
            field
                .as_bytes()
                .windows(word.len())
                .any(|part| part.eq_ignore_ascii_case(word.as_bytes()))
        })
    })
}

pub fn matching(query: &str) -> impl Iterator<Item = &'static Action> {
    ACTIONS
        .iter()
        .filter(move |action| action.name != "none" && matches(query, &[action.name, action.label]))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub symbol: u64,
    pub modifiers: u32,
    pub action: &'static str,
}

impl Binding {
    fn chord(&self, display: bool) -> String {
        let mut text = String::new();
        for (bit, name, label) in [
            (4, "ctrl", "Ctrl"),
            (1, "shift", "Shift"),
            (8, "alt", "Alt"),
            (64, "super", "Super"),
        ] {
            if self.modifiers & bit != 0 {
                text.push_str(if display { label } else { name });
                text.push('+');
            }
        }
        let key = match self.symbol {
            0xff0d => "enter",
            0xff1b => "escape",
            0xff09 => "tab",
            0xff08 => "backspace",
            0x20 => "space",
            0xff55 => "pageup",
            0xff56 => "pagedown",
            0xff51 => "left",
            0xff53 => "right",
            0xff52 => "up",
            0xff54 => "down",
            0xff50 => "home",
            0xff57 => "end",
            0x3d => "equal",
            0x2d => "minus",
            _ => "",
        };
        if !key.is_empty() {
            if display {
                for (index, ch) in key.chars().enumerate() {
                    text.push(if index == 0 {
                        ch.to_ascii_uppercase()
                    } else {
                        ch
                    });
                }
            } else {
                text.push_str(key);
            }
        } else if (0x1000_0001..=0x1000_000b).contains(&self.symbol) {
            text.push_str(&format!(
                "{}{}",
                if display { "Button" } else { "button" },
                self.symbol - 0x1000_0000
            ));
        } else if (0xffbe..=0xffe0).contains(&self.symbol) {
            text.push_str(&format!(
                "{}{}",
                if display { "F" } else { "f" },
                self.symbol - 0xffbd
            ));
        } else if let Some(ch) = char::from_u32(self.symbol as u32) {
            text.push(if display { ch.to_ascii_uppercase() } else { ch });
        }
        text
    }

    pub fn config(&self) -> String {
        let mut text = self.chord(false);
        text.push_str(" = ");
        text.push_str(self.action);
        text
    }
}

pub fn canonical(symbol: u64) -> u64 {
    match symbol {
        0x41..=0x5a => symbol + 32,
        0x2b => 0x3d,
        0x5f => 0x2d,
        _ => symbol,
    }
}

pub fn parse(value: &str, mouse: bool) -> Result<Binding, &'static str> {
    let (keys, action) = value.rsplit_once('=').ok_or("expected CHORD = ACTION")?;
    let action = ACTIONS
        .iter()
        .find(|a| a.name == action.trim())
        .ok_or("unknown view action")?
        .name;
    let mut modifiers = 0;
    let mut symbol = None;
    for part in keys.trim().split('+') {
        let part = part.trim();
        let bit = match part {
            "ctrl" => Some(4),
            "shift" => Some(1),
            "alt" => Some(8),
            "super" => Some(64),
            _ => None,
        };
        if let Some(bit) = bit {
            if modifiers & bit != 0 {
                return Err("duplicate binding modifier");
            }
            modifiers |= bit;
            continue;
        }
        if symbol.is_some() {
            return Err("a binding must contain exactly one key or button");
        }
        let key = if mouse {
            0x1000_0000
                + part
                    .strip_prefix("button")
                    .and_then(|s| s.parse::<u64>().ok())
                    .filter(|n| (1..=11).contains(n))
                    .ok_or("mouse key must be button1..button11")?
        } else {
            match part {
                "enter" => 0xff0d,
                "escape" => 0xff1b,
                "tab" => 0xff09,
                "backspace" => 0xff08,
                "space" => 0x20,
                "pageup" => 0xff55,
                "pagedown" => 0xff56,
                "left" => 0xff51,
                "right" => 0xff53,
                "up" => 0xff52,
                "down" => 0xff54,
                "home" => 0xff50,
                "end" => 0xff57,
                "plus" | "equal" => 0x3d,
                "minus" => 0x2d,
                _ if part.len() == 1 && part.as_bytes()[0].is_ascii_graphic() => {
                    canonical(u64::from(part.as_bytes()[0]))
                }
                _ => {
                    0xffbd
                        + part
                            .strip_prefix('f')
                            .and_then(|s| s.parse::<u64>().ok())
                            .filter(|n| (1..=35).contains(n))
                            .ok_or("unknown binding key")?
                }
            }
        };
        symbol = Some(key);
    }
    Ok(Binding {
        symbol: symbol.ok_or("binding key is missing")?,
        modifiers,
        action,
    })
}

pub fn set(bindings: &mut Vec<Binding>, binding: Binding) -> Result<(), &'static str> {
    if let Some(old) = bindings
        .iter_mut()
        .find(|b| b.symbol == binding.symbol && b.modifiers == binding.modifiers)
    {
        *old = binding;
    } else {
        if bindings.len() >= 128 {
            return Err("at most 128 custom bindings are supported");
        }
        bindings.push(binding);
    }
    Ok(())
}

pub fn lookup(bindings: &[Binding], symbol: u64, state: u32) -> Option<&'static str> {
    let symbol = canonical(symbol);
    let state = state & (1 | 4 | 8 | 64);
    bindings
        .iter()
        .rev()
        .find(|b| b.symbol == symbol && b.modifiers == state)
        .map(|b| b.action)
}

/// Prefer an explicitly bound shifted symbol, then retain physical-key
/// bindings and defaults. The frontend supplies its active layout's symbol.
pub fn key_action_with_shifted(
    bindings: &[Binding],
    symbol: u64,
    shifted: Option<u64>,
    state: u32,
) -> Option<&'static str> {
    shifted
        .filter(|_| state & 1 != 0)
        .and_then(|symbol| lookup(bindings, symbol, state))
        .or_else(|| key_action(bindings, symbol, state))
}

pub fn key_action(bindings: &[Binding], symbol: u64, state: u32) -> Option<&'static str> {
    if let Some(action) = lookup(bindings, symbol, state) {
        return Some(action);
    }
    let symbol = canonical(symbol);
    let state = state & (1 | 4 | 8 | 64);
    DEFAULT_KEYS
        .iter()
        .find(|&&(key, modifiers, _)| key == symbol && modifiers == state)
        .map(|&(_, _, action)| action)
}

// One table serves both dispatch and command-palette hints, so a disabled or
// remapped default never appears as an active shortcut.
const DEFAULT_KEYS: &[(u64, u32, &str)] = &[
    (0xff55, 1, "scroll-up"),
    (0xff56, 1, "scroll-down"),
    (0x3d, 4, "font-increase"),
    (0x2d, 4, "font-decrease"),
    (0x30, 4, "font-reset"),
    (0x74, 5, "new-tab"),
    (0x65, 5, "split-right"),
    (0x6f, 5, "split-down"),
    (0x6e, 5, "new-window"),
    (0x75, 5, "move-window"),
    (0x64, 5, "detach"),
    (0x77, 5, "close"),
    (0x67, 5, "take-control"),
    (0x66, 5, "search"),
    (0x70, 5, "palette"),
    (0x73, 5, "sessions"),
    (0x63, 5, "copy"),
    (0x76, 5, "paste"),
    (0x72, 5, "reload"),
    (0x6b, 5, "previous-command"),
    (0x6a, 5, "next-command"),
    (0x79, 5, "copy-command-output"),
    (0xff55, 5, "previous-tab"),
    (0xff56, 5, "next-tab"),
    (0xff51, 5, "previous-pane"),
    (0xff53, 5, "next-pane"),
    (0xff52, 5, "swap-previous"),
    (0xff54, 5, "swap-next"),
    (0xff0d, 5, "maximize"),
    (0x3d, 5, "grow"),
    (0x2d, 5, "shrink"),
];

/// A currently effective custom shortcut, falling back to an active default.
pub fn shortcut(bindings: &[Binding], action: &str) -> Option<String> {
    bindings
        .iter()
        .rev()
        .copied()
        .find(|binding| {
            binding.action == action
                && lookup(bindings, binding.symbol, binding.modifiers) == Some(binding.action)
        })
        .or_else(|| {
            DEFAULT_KEYS.iter().find_map(|&(symbol, modifiers, name)| {
                (name == action && key_action(bindings, symbol, modifiers) == Some(name)).then_some(
                    Binding {
                        symbol,
                        modifiers,
                        action: name,
                    },
                )
            })
        })
        .map(|binding| binding.chord(true))
}
