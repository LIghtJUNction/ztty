//! Input ownership shared by terminal frontends. No transport or UI side effects.
use alloc::string::String;

pub fn valid_actor(actor: &str) -> bool {
    !actor.is_empty()
        && actor.len() <= 128
        && actor
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

/// Actors are validated by the request boundary before they reach this state.
pub struct SessionControl {
    owner: Option<String>,
}

impl SessionControl {
    pub fn new(owner: String) -> Self {
        Self { owner: Some(owner) }
    }

    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    pub fn owns(&self, actor: &str) -> bool {
        self.owner() == Some(actor)
    }

    /// Return true only when pending input from the previous owner must be
    /// cancelled. Reacquiring one's own session preserves its pending input.
    pub fn acquire(&mut self, actor: &str, takeover: bool) -> Result<bool, &'static str> {
        if self.owner().is_some_and(|owner| owner != actor) && !takeover {
            return Err("session already has an input owner; explicit takeover is required");
        }
        let changed = !self.owns(actor);
        if changed {
            self.owner = Some(actor.into());
        }
        Ok(changed)
    }

    /// A successful release always requires cancelling pending input.
    pub fn release(&mut self, actor: &str) -> Result<(), &'static str> {
        if !self.owns(actor) {
            return Err("only the input owner can release control");
        }
        self.owner = None;
        Ok(())
    }
}
