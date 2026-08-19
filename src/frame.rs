//! Wire format framing data used by both server and clients respectively.
//!
//! # Design
//!
//! Rather than opening one persistent iroh stream per `bevy_replicon` channel
//! which would need a stream-identification handshake, as iroh's own docs note
//! that peer `accept_uni()` only yields a stream once data has actually been
//! written to it. So accept() order isn't guaranteed to match open() order like
//! most network protocols, we cheat and multiplex all of replicon's
//! `Ordered`/`Unordered` channels for a single direction onto a single
//! long-lived stream instead. It made the most sense to me and can be changed
//! if its a dum idea in the end.
//!
//! - server -> client: one stream, carries all `Ordered`/`Unordered`
//!   `ServerChannel` data.
//! - client -> server: one stream, carries all `Ordered`/`Unordered`
//!   `ClientChannel` data too (separate return channel though here).
//!
//! This is enough to give `bevy_replicon`'s own reserved channel traffic one
//! shared high-priority tier [`crate::REPLICON_STREAM_PRIORITY`] without
//! needing per-channel stream negotiation which was really annoying and seems
//! unnecessary. Applications that want additional, independently-prioritized
//! streams e.g. say bulk data transfer for file data can open their own
//! directly via `iroh::endpoint::Connection`, entirely outside this framing
//! setup.
//!
//! `Unreliable` channels don't use a stream at all they ride iroh's native
//! datagrams, which already preserve message boundaries. You can use that too
//! if you want. I've no idea what you'd use it for but shrug, the Iroh endpoint
//! gives you the option to do weird stuff like the author go nuts! Use a
//! carrier pigeon to transfer the bytes, mabye a asthmatic squirrel. The world
//! is your oyster, or least weasel.
//!
//! ## Stream framing
//!
//! The first byte written to and required to be the first byte read, and
//! required to be the first byte on the receiving side for every persistent
//! stream is `STREAM_MARKER` NB: private to the crate. Its only purpose is
//! forcing bytes onto the wire immediately when the stream is opened, so that
//! the Iroh peer's `accept_uni()` resolves closer to open() time instead of
//! waiting on whenever the first real application message happens to be
//! queued/sent which matters for short-lived clients that want to observe/act
//! on the connection quickly after establishing it. An example might be a cli
//! command talking to a local daemon, it needs data sooner but the channel
//! should be there before that data arrives even if that is 100ms later. This
//! is mostly an optimization for short lived data connections.
//!
//! After the marker data, the stream is just a sequence of frames as:
//! `[channel_id: u8][len: u32 le][payload: len bytes]` nothing interesting.
//!
//! ## Datagram framing
//!
//! Stupid simple for now: `[channel_id: u8][payload]` No length prefix as
//! datagrams already preserve message boundaries implicitly The channel_id
//! prefix exists because more than one `Unreliable` channel could exist on an
//! Iroh connection.

use bytes::Bytes;
use iroh::endpoint::{RecvStream, SendStream};

/// Stream marker magic byte. Value pulled out of my butt but shouldn't conflict with anything I found in Iroh.
pub const STREAM_MARKER: u8 = 0xB7;

/// Ensure [`STREAM_MARKER`] is the first byte of a brand spankin new stream.
/// Public as applications opening their own extra streams on a shared
/// `Connection`/extra-ALPN connection may want the same "force some bytes onto
/// the wire immediately" hack my cli use case did.
pub async fn write_marker(stream: &mut SendStream) -> anyhow::Result<()> {
    stream.write_all(&[STREAM_MARKER]).await?;
    Ok(())
}

/// Read and validate [`STREAM_MARKER`] is the first byte of a stream.
pub async fn read_marker(stream: &mut RecvStream) -> anyhow::Result<()> {
    let mut buf = [0u8; 1];
    stream.read_exact(&mut buf).await?;
    anyhow::ensure!(
        buf[0] == STREAM_MARKER,
        "unexpected stream marker byte {:#x}, expected {:#x} peer appears to be running an incompatible protocol version, sent invalid data, or the stream itself is corrupted",
        buf[0],
        STREAM_MARKER
    );
    Ok(())
}

/// Write a `[channel_id][len][payload]` frame. Public for anything
/// building its own streams/connections not really replicon-specific.
pub async fn write_frame(
    stream: &mut SendStream,
    channel_id: u8,
    payload: &[u8],
) -> anyhow::Result<()> {
    let len = payload.len() as u32;
    let mut header = [0u8; 5];
    header[0] = channel_id;
    header[1..5].copy_from_slice(&len.to_le_bytes());
    stream.write_all(&header).await?;
    if !payload.is_empty() {
        stream.write_all(payload).await?;
    }
    Ok(())
}

/// Read one `[channel_id][len][payload]` frame. Returns `None` once the stream
/// ends via the peer finish send(), stream reset/RST, or connection died
/// somehow.
///
/// Callers must treat `None` as "stop reading this stream", not
/// necessarily as a fatal error. Overall connection health is tracked
/// separately, that's why this isn't a `Result`.
///
/// The intent here is to allow callers to "do weird stuff" in Iroh like
/// multiple paths and retry on their own if wanted to mimic things like
/// multipath tcp.
pub async fn read_frame(stream: &mut RecvStream) -> Option<(u8, Bytes)> {
    let mut header = [0u8; 5];
    if let Err(e) = stream.read_exact(&mut header).await {
        tracing::debug!("persistent stream ended: {e}");
        return None;
    }
    let channel_id = header[0];
    let len = u32::from_le_bytes(header[1..5].try_into().unwrap()) as usize;
    let mut payload = vec![0u8; len];
    if len > 0
        && let Err(e) = stream.read_exact(&mut payload).await
    {
        tracing::debug!("persistent stream unexpectedly ended mid-frame: {e}");
        return None;
    }
    Some((channel_id, Bytes::from(payload)))
}

/// Encode a datagram `[channel_id][payload]`.
pub(crate) fn encode_datagram(channel_id: u8, payload: &[u8]) -> Bytes {
    let mut buf = Vec::with_capacity(1 + payload.len());
    buf.push(channel_id);
    buf.extend_from_slice(payload);
    Bytes::from(buf)
}

/// Decode a datagram back into `(channel_id, payload)`. Returns `None` for an
/// empty datagram, which shouldn't happen from a well-behaved peer but avoids a
/// panic if it did. For example: someone like the author mangling packets live
/// on the wire to test lossy-link behavior via tc in a nixos stress test that
/// isn't committed yet.
pub(crate) fn decode_datagram(datagram: &Bytes) -> Option<(u8, Bytes)> {
    if datagram.is_empty() {
        return None;
    }
    let channel_id = datagram[0];
    let payload = datagram.slice(1..);
    Some((channel_id, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datagram_roundtrips() {
        let encoded = encode_datagram(7, b"hello");
        let (channel_id, payload) = decode_datagram(&encoded).unwrap();
        assert_eq!(channel_id, 7);
        assert_eq!(&payload[..], b"hello");
    }

    #[test]
    fn datagram_roundtrips_empty_payload() {
        let encoded = encode_datagram(3, b"");
        let (channel_id, payload) = decode_datagram(&encoded).unwrap();
        assert_eq!(channel_id, 3);
        assert!(payload.is_empty());
    }

    #[test]
    fn empty_datagram_decodes_to_none() {
        assert!(decode_datagram(&Bytes::new()).is_none());
    }
}
