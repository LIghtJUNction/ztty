//! Linux host selection. A complete view must initialize before a PTY is started.
use alloc::{format, string::String};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    X11,
    Wayland,
}

impl Backend {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        match value {
            "x11" => Ok(Self::X11),
            "wayland" => Ok(Self::Wayland),
            _ => Err("invalid saved view backend"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::X11 => "x11",
            Self::Wayland => "wayland",
        }
    }

    pub fn from_checkpoint(
        previous: Option<&str>,
        current: Option<&str>,
    ) -> Result<Option<Self>, &'static str> {
        let previous = previous.map(Self::parse).transpose()?;
        let current = current.map(Self::parse).transpose()?;
        if let (Some(previous), Some(current)) = (previous, current)
            && previous != current
        {
            return Err("saved view backend does not match its attached window");
        }
        // Older checkpoints have no provenance field. Their attached X11 view
        // is still evidence; a detached session's old backend remains unknown.
        Ok(current.or(previous))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Preference {
    #[default]
    Auto,
    X11,
    Wayland,
}

impl Preference {
    pub fn parse(value: &[u8]) -> Result<Self, &'static str> {
        match value {
            b"auto" => Ok(Self::Auto),
            b"x11" => Ok(Self::X11),
            b"wayland" => Ok(Self::Wayland),
            _ => Err("--backend must be auto, x11 or wayland"),
        }
    }

    /// Older brokers understand this concrete name and ignore the separate
    /// optional `_backend_auto` flag. Never send them `_backend: "auto"`.
    pub fn legacy_backend(self) -> Backend {
        match self {
            Self::Auto | Self::X11 => Backend::X11,
            Self::Wayland => Backend::Wayland,
        }
    }

    pub fn for_attachment(self, previous: Option<Backend>) -> Self {
        match (self, previous) {
            (Self::Auto, Some(Backend::X11)) => Self::X11,
            (Self::Auto, Some(Backend::Wayland)) => Self::Wayland,
            _ => self,
        }
    }
}

pub struct Requirements {
    pub workspace: bool,
    pub native_compatible: bool,
    pub native_compiled: bool,
}

/// Retry only full native-view initialization, never session creation or PTY
/// operations. X11 currently supplies the complete workspace/reload feature set.
pub fn initialize<T>(
    preference: Preference,
    requirements: Requirements,
    mut open: impl FnMut(Backend) -> Result<T, String>,
) -> Result<T, String> {
    if preference == Preference::Wayland && requirements.workspace {
        return Err("Wayland workspaces are not available; use independent session windows".into());
    }
    if preference != Preference::Auto {
        return open(preference.legacy_backend());
    }
    let x11_error = match open(Backend::X11) {
        Ok(window) => return Ok(window),
        Err(error) => error,
    };
    let blocked = if requirements.workspace {
        Some("tabs and splits require X11; native Wayland fallback is unavailable")
    } else if !requirements.native_compatible {
        Some(
            "native Wayland fallback would disable runtime reload for existing sessions; existing sessions were kept",
        )
    } else if !requirements.native_compiled {
        Some("native Wayland is optional; build with --features wayland")
    } else {
        None
    };
    if let Some(reason) = blocked {
        return Err(format!(
            "automatic backend: X11 failed ({x11_error}); {reason}"
        ));
    }
    open(Backend::Wayland).map_err(|error| {
        format!("automatic backend: X11 failed ({x11_error}); Wayland failed ({error})")
    })
}
