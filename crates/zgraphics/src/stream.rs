use alloc::vec::Vec;

use crate::{
    Context, Graphics, Outcome,
    protocol::{self, Command},
};

/// Bounded streaming decoder for a complete graphics APC body.
///
/// Some deployed clients emit a whole image in one APC instead of the protocol's
/// recommended 4096-byte chunks. This adapter retains at most a 512-byte header
/// and 4096 encoded payload bytes, forwarding decoded blocks to the same bounded
/// image transfer. The host strips ESC _ / ST and calls `finish` only at ST.
/// No image becomes visible until that final call, using its cursor context.
pub struct CommandStream {
    header: Vec<u8>,
    payload: Vec<u8>,
    deferred: Option<Command>,
    separated: bool,
    transmitting: bool,
    more: bool,
    ignored: bool,
}

impl Default for CommandStream {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandStream {
    pub const fn new() -> Self {
        Self {
            header: Vec::new(),
            payload: Vec::new(),
            deferred: None,
            separated: false,
            transmitting: false,
            more: false,
            ignored: false,
        }
    }

    /// Begin the next APC. An existing protocol m=1 transfer remains pending.
    pub fn begin(&mut self) {
        self.header.clear();
        self.payload.clear();
        self.deferred = None;
        self.separated = false;
        self.transmitting = false;
        self.more = false;
        self.ignored = false;
    }

    /// Number of encoded header/payload bytes currently retained by this adapter.
    pub fn buffered_bytes(&self) -> usize {
        self.header.len() + self.payload.len()
    }

    /// Cancel an unterminated APC and its in-progress image transfer.
    pub fn abort(&mut self, graphics: &mut Graphics) {
        self.begin();
        self.ignored = true;
        graphics.abort_transfer();
    }

    /// Consume arbitrary fragments of an APC body, starting with its `G` marker.
    /// This never returns a placement: it can only appear at `finish`.
    pub fn push(
        &mut self,
        graphics: &mut Graphics,
        mut bytes: &[u8],
        context: Context,
        replies: &mut Vec<u8>,
    ) {
        if self.ignored {
            return;
        }
        if !self.separated {
            let separator = bytes.iter().position(|b| *b == b';');
            let end = separator.unwrap_or(bytes.len());
            if self.header.is_empty() && bytes.first().is_some_and(|b| *b != b'G') {
                self.ignored = true;
                return;
            }
            if self.header.len().saturating_add(end) > 512 {
                self.abort(graphics);
                return;
            }
            self.header.extend_from_slice(&bytes[..end]);
            let Some(_) = separator else {
                return;
            };
            bytes = &bytes[end + 1..];
            self.separated = true;
            let Ok((mut command, _)) = protocol::parse(&self.header) else {
                self.abort(graphics);
                return;
            };
            self.more = command.more;
            self.transmitting =
                matches!(command.action, b't' | b'T' | b'q') && command.feature == 0;
            if self.transmitting {
                // Validate dimensions/transport and install metadata immediately,
                // without retaining or decoding the entire encoded APC body.
                command.more = true;
                graphics.process(command, &[], context, replies);
                if graphics.pending.is_none() {
                    self.ignored = true;
                    return;
                }
            } else {
                self.deferred = Some(command);
            }
        }
        if !self.transmitting {
            return;
        }
        if self.payload.capacity() < 4096
            && self
                .payload
                .try_reserve_exact(4096 - self.payload.len())
                .is_err()
        {
            graphics.reject_transfer("ENOMEM:cannot allocate graphics staging buffer", replies);
            self.ignored = true;
            return;
        }
        while !bytes.is_empty() {
            if self.payload.len() == 4096 {
                // Keep one full final block buffered until another byte arrives:
                // a padded final block must not be mistaken for m=1 data.
                if self.payload.contains(&b'=') {
                    graphics.reject_transfer("EINVAL:padding before final graphics block", replies);
                    self.ignored = true;
                    self.payload.clear();
                    return;
                }
                graphics.process(
                    Command {
                        more: true,
                        ..Command::default()
                    },
                    &self.payload,
                    context,
                    replies,
                );
                self.payload.clear();
                if graphics.pending.is_none() {
                    self.ignored = true;
                    return;
                }
            }
            let take = (4096 - self.payload.len()).min(bytes.len());
            self.payload.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
        }
    }

    /// Finish this APC at ST. The original m flag controls whether the image is
    /// complete or another APC must follow. Unpadded final base64 is accepted.
    pub fn finish(
        &mut self,
        graphics: &mut Graphics,
        context: Context,
        replies: &mut Vec<u8>,
    ) -> Outcome {
        let outcome = if self.ignored {
            Outcome::default()
        } else if !self.separated {
            graphics.command(&self.header, context, replies)
        } else if self.transmitting {
            if self.more && (!self.payload.len().is_multiple_of(4) || self.payload.contains(&b'='))
            {
                graphics.reject_transfer("EINVAL:invalid nonfinal base64 chunk", replies);
                Outcome::default()
            } else {
                graphics.process(
                    Command {
                        more: self.more,
                        ..Command::default()
                    },
                    &self.payload,
                    context,
                    replies,
                )
            }
        } else if let Some(command) = self.deferred.take() {
            graphics.process(command, &[], context, replies)
        } else {
            Outcome::default()
        };
        self.begin();
        outcome
    }
}
