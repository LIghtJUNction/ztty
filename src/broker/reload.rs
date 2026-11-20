//! Preserve existing management and wait connections through same-PID exec.
use super::*;
use crate::runtime_reload::{self as wire, boolean, bytes, descriptor, list, number};

impl Server {
    pub fn checkpoint_reload(&self) -> Value {
        json::object([
            ("fd", self.fd.0.into()),
            ("checkpoint_at", os::milliseconds().into()),
            (
                "peers",
                Value::Array(
                    self.peers
                        .iter()
                        .map(|peer| {
                            json::object([
                                ("fd", peer.fd.0.into()),
                                ("input", hex(&peer.input).into()),
                                ("output", hex(&peer.output).into()),
                                ("sent", peer.sent.into()),
                                (
                                    "wait",
                                    peer.wait.as_deref().map(Value::from).unwrap_or(Value::Null),
                                ),
                                (
                                    "read",
                                    peer.read_wait
                                        .as_ref()
                                        .map(|read| {
                                            json::object([
                                                ("session", read.session_id.into()),
                                                ("cursor", read.cursor.into()),
                                                (
                                                    "max",
                                                    read.max_bytes
                                                        .map(Value::from)
                                                        .unwrap_or(Value::Null),
                                                ),
                                                ("deadline", read.deadline.into()),
                                            ])
                                        })
                                        .unwrap_or(Value::Null),
                                ),
                                ("deadline", peer.deadline.into()),
                                ("done", peer.done.into()),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }

    pub fn validate_reload(value: &Value) -> Result<Vec<i32>, String> {
        let mut fds = alloc::vec![descriptor(value, "fd")?];
        number(value, "checkpoint_at")?;
        let peers = list(value, "peers")?;
        if peers.len() > MAX_PEERS {
            return Err("reload peer limit exceeded".into());
        }
        for peer in peers {
            let fd = descriptor(peer, "fd")?;
            if fds.contains(&fd) {
                return Err("duplicate reload socket descriptor".into());
            }
            fds.push(fd);
            bytes(peer, "input", LIMIT)?;
            let output = bytes(peer, "output", LIMIT * 2)?;
            if number(peer, "sent")? > output.len() as u64 {
                return Err("invalid reload reply offset".into());
            }
            number(peer, "deadline")?;
            boolean(peer, "done")?;
            if let Some(wait) = peer.get("wait").filter(|v| **v != Value::Null)
                && wait.as_str().is_none_or(|s| s.len() > 4096)
            {
                return Err("invalid reload waiter".into());
            }
            if let Some(read) = peer.get("read").filter(|v| **v != Value::Null) {
                number(read, "session")?;
                number(read, "cursor")?;
                number(read, "deadline")?;
                if read.get("max").is_some_and(|v| {
                    *v != Value::Null && v.as_u64().is_none_or(|n| n == 0 || n > 65536)
                }) {
                    return Err("invalid reload read size".into());
                }
            }
        }
        Ok(fds)
    }

    /// The checkpoint was decoded in the validator before exec. The inherited
    /// socket descriptors belong uniquely to this resumed broker.
    ///
    /// # Safety
    /// Every saved descriptor must still be the original broker-owned socket,
    /// with no other Rust owner, in the same process after executable replacement.
    pub unsafe fn restore_reload(value: &Value) -> Result<Self, String> {
        Self::validate_reload(value)?;
        let paused = os::milliseconds().saturating_sub(number(value, "checkpoint_at")?);
        let mut peers = Vec::new();
        for peer in list(value, "peers")? {
            let read_wait = match peer.get("read").filter(|v| **v != Value::Null) {
                Some(read) => Some(ReadWait {
                    session_id: number(read, "session")?,
                    cursor: number(read, "cursor")?,
                    max_bytes: read.get("max").and_then(Value::as_u64),
                    deadline: number(read, "deadline")?,
                }),
                None => None,
            };
            peers.push(Peer {
                fd: Fd(descriptor(peer, "fd")?),
                input: bytes(peer, "input", LIMIT)?,
                output: bytes(peer, "output", LIMIT * 2)?,
                sent: number(peer, "sent")? as usize,
                wait: peer.get("wait").and_then(Value::as_str).map(String::from),
                read_wait,
                deadline: number(peer, "deadline")?.saturating_add(paused),
                done: boolean(peer, "done")?,
            });
        }
        Ok(Self {
            fd: Fd(descriptor(value, "fd")?),
            peers,
        })
    }

    pub fn finish_reload(&mut self, result: Result<Value, String>) {
        for peer in &mut self.peers {
            if peer.wait.as_deref() == Some(wire::WAIT) {
                peer.reply(result.clone());
            }
        }
    }
}
