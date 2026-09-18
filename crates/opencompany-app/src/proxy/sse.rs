//! Reassembling `text/event-stream` frames from a byte stream.
//!
//! The browser gets this for free from `EventSource`. The desktop cannot use
//! `EventSource` at all — it cannot set a request header, so it cannot carry
//! the session header a paired device authenticates with, and a `SameSite=Lax`
//! cookie is never sent cross-site either. So the desktop reads the stream with
//! an ordinary HTTP client, which means parsing the wire format by hand.
//!
//! Deliberately a *parser over chunks*, not over lines: a TCP read boundary
//! falls wherever it falls, and it will eventually fall in the middle of a JSON
//! payload. A line-oriented reader that assumed each chunk was whole would work
//! for months and then drop one event under load, which is the kind of bug
//! nobody traces back to here.
//!
//! Only what the console consumes is implemented. `event:` names, `id:` and
//! `retry:` are parsed far enough to be *ignored correctly* — the host emits
//! only default-type messages, and silently treating a named event as a default
//! one would be worse than dropping it.

/// Accumulates bytes and yields complete event payloads.
#[derive(Debug, Default)]
pub struct SseDecoder {
    /// Bytes received but not yet forming a complete event.
    buffer: String,
    /// Bytes that are not yet a whole codepoint. See [`SseDecoder::push_bytes`].
    partial: Vec<u8>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds a chunk of **bytes**, returning every event completed by it.
    ///
    /// This is the entry point a network reader wants, and the reason it exists
    /// is the same reason this is a chunk decoder at all — one level down.
    /// Framing is chunk-safe, but decoding each chunk with
    /// `String::from_utf8_lossy` before framing is not: a read boundary lands
    /// wherever the network puts it, and eventually that is *inside* a
    /// multi-byte codepoint. Each half then decodes to U+FFFD independently, so
    /// an agent reply containing an emoji or any accented text arrives silently
    /// mangled — under load, occasionally, in a payload nobody can reproduce.
    ///
    /// So incomplete trailing bytes are held here until the chunk that
    /// completes them. Bytes that are genuinely malformed — as opposed to
    /// merely truncated — still become U+FFFD, exactly as before, because
    /// waiting for the rest of a sequence that is never coming would stall the
    /// stream.
    pub fn push_bytes(&mut self, chunk: &[u8]) -> Vec<String> {
        self.partial.extend_from_slice(chunk);
        let decoded = self.take_decodable();
        self.push(&decoded)
    }

    /// The longest prefix of `partial` that is whole codepoints, consuming it.
    fn take_decodable(&mut self) -> String {
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.partial) {
                Ok(all) => {
                    out.push_str(all);
                    self.partial.clear();
                    return out;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    // Sound by construction: `valid_up_to` is where decoding
                    // stopped, so everything before it is valid UTF-8.
                    out.push_str(std::str::from_utf8(&self.partial[..valid]).unwrap_or_default());
                    match error.error_len() {
                        // A truncated codepoint at the tail. The rest of it is
                        // in the next chunk, so keep the bytes rather than
                        // replacing them. At most three are ever held.
                        None => {
                            self.partial.drain(..valid);
                            return out;
                        }
                        // Genuinely malformed, not split. Mirror what
                        // `from_utf8_lossy` would have produced and continue,
                        // so one bad sequence does not stall the stream.
                        Some(len) => {
                            out.push(char::REPLACEMENT_CHARACTER);
                            self.partial.drain(..valid + len);
                        }
                    }
                }
            }
        }
    }

    /// Feeds a chunk of text, returning every event completed by it.
    ///
    /// A chunk may complete several events, part of one, or none. Prefer
    /// [`push_bytes`](Self::push_bytes) when reading from a socket; this is for
    /// callers that already hold text.
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.buffer.push_str(chunk);
        let mut out = Vec::new();

        // An event ends at a blank line. `\r\n` is legal in the format and some
        // proxies rewrite line endings, so both are accepted.
        while let Some((frame, rest)) = split_frame(&self.buffer) {
            if let Some(data) = decode_frame(&frame) {
                out.push(data);
            }
            self.buffer = rest;
        }
        out
    }
}

/// Splits the first complete frame off `buffer`, if there is one.
fn split_frame(buffer: &str) -> Option<(String, String)> {
    let candidates = ["\r\n\r\n", "\n\n", "\r\r"];
    let (index, terminator) = candidates
        .iter()
        .filter_map(|t| buffer.find(t).map(|i| (i, *t)))
        .min_by_key(|(i, _)| *i)?;
    let frame = buffer[..index].to_string();
    let rest = buffer[index + terminator.len()..].to_string();
    Some((frame, rest))
}

/// The `data` payload of one frame, or `None` when there is nothing to deliver.
fn decode_frame(frame: &str) -> Option<String> {
    let mut data: Vec<&str> = Vec::new();
    let mut named_event: Option<&str> = None;

    for line in frame.split(['\n', '\r']).filter(|l| !l.is_empty()) {
        // A line starting with `:` is a comment. Hosts send these as keep-alive
        // pings, and treating one as data would hand the console a payload it
        // cannot parse on every heartbeat.
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            // A bare field name with no colon is a field with an empty value.
            None => (line, ""),
        };
        match field {
            "data" => data.push(value),
            "event" => named_event = Some(value),
            // `id` and `retry` are meaningful to a reconnecting client. This one
            // does not resume by `Last-Event-ID` — the console's poll is the
            // safety net — so they are read and dropped rather than
            // misinterpreted.
            _ => {}
        }
    }

    // The console subscribes to default-type messages only, which is what
    // `EventSource.onmessage` delivers. Passing a named event through here would
    // silently promote it to a message the caller never asked for.
    if named_event.is_some_and(|name| name != "message") {
        return None;
    }
    if data.is_empty() {
        return None;
    }
    Some(data.join("\n"))
}

#[cfg(test)]
#[path = "sse_tests.rs"]
mod tests;
