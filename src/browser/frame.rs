//! The frames on `browser.sock` (kai-firefox design §4.1): native messaging's own framing, a
//! 4-byte little-endian length and that many bytes of UTF-8 JSON, which `kai-browser-host`
//! relays to and from Firefox unchanged.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The most Firefox takes in one message toward the extension: the body's length in UTF-8 bytes,
/// this size included (the default of Firefox's pref
/// `webextensions.native-messaging.max-input-message-bytes`, `MAX_READ` in its
/// `NativeMessaging.sys.mjs`; one byte more ends the port, measured in Firefox 159). Every frame
/// Kai sends is at most this long: a longer one goes as `chunk` frames (`super::chunk`). Toward
/// Kai a frame may be as long as its length field says (4 GiB), as Firefox allows; the extension
/// splits its long ones the same way, since Firefox writes one large message slowly.
pub const TO_FIREFOX_MAX: usize = 1024 * 1024;

/// One frame's body; `None` when the stream ended cleanly between frames. A stream that ends
/// inside a frame is an error. The body is read as it arrives, so memory follows what was sent,
/// not what the length claims.
pub async fn read(r: &mut (impl AsyncRead + Unpin)) -> std::io::Result<Option<Vec<u8>>> {
    match read_len(r).await? {
        Some(len) => read_body(r, len).await.map(Some),
        None => Ok(None),
    }
}

/// A frame's length; `None` when the stream ended cleanly before it.
pub async fn read_len(r: &mut (impl AsyncRead + Unpin)) -> std::io::Result<Option<u32>> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < len.len() {
        match r.read(&mut len[got..]).await? {
            0 if got == 0 => return Ok(None),
            0 => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "the stream ended inside a frame's length",
                ));
            }
            n => got += n,
        }
    }
    Ok(Some(u32::from_le_bytes(len)))
}

/// A frame's body of `len` bytes (after [`read_len`]), read as it arrives.
pub async fn read_body(r: &mut (impl AsyncRead + Unpin), len: u32) -> std::io::Result<Vec<u8>> {
    let len = u64::from(len);
    let mut body = Vec::new();
    r.take(len).read_to_end(&mut body).await?;
    if body.len() as u64 != len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!(
                "the stream ended inside a frame of {len} bytes ({} came)",
                body.len()
            ),
        ));
    }
    Ok(body)
}

/// Writes one frame. A body longer than the length field can say (4 GiB) is an error.
pub async fn write(w: &mut (impl AsyncWrite + Unpin), body: &[u8]) -> std::io::Result<()> {
    let len = u32::try_from(body.len()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "a frame of {} bytes is longer than a frame's length can say (4 GiB)",
                body.len()
            ),
        )
    })?;
    w.write_all(&len.to_le_bytes()).await?;
    w.write_all(body).await?;
    w.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_round_trip_and_end_cleanly_only_between_frames() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let bodies: Vec<Vec<u8>> = vec![
            br#"{"t":"hello"}"#.to_vec(),
            Vec::new(),
            "{\"t\":\"x\",\"s\":\"\u{e4}\u{1f600}\"}"
                .as_bytes()
                .to_vec(),
            vec![b'a'; 300],
        ];
        let sent = bodies.clone();
        let writer = tokio::spawn(async move {
            for body in &sent {
                write(&mut a, body).await.unwrap();
            }
        });
        for body in &bodies {
            assert_eq!(
                read(&mut b).await.unwrap().as_deref(),
                Some(body.as_slice())
            );
        }
        writer.await.unwrap();
        assert_eq!(read(&mut b).await.unwrap(), None, "a clean end");

        // The wire: the length little-endian, then the body.
        let mut out = Vec::new();
        write(&mut out, b"{}").await.unwrap();
        assert_eq!(out, [2, 0, 0, 0, b'{', b'}']);
    }

    #[tokio::test]
    async fn a_stream_that_ends_inside_a_frame_is_an_error() {
        let mut half_length: &[u8] = &[5, 0];
        let e = read(&mut half_length).await.unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof);
        let mut short_body: &[u8] = &[5, 0, 0, 0, b'{', b'}'];
        let e = read(&mut short_body).await.unwrap_err();
        assert!(e.to_string().contains("a frame of 5 bytes (2 came)"), "{e}");
        // A length near 4 GiB with nothing behind it allocates nothing up front.
        let mut huge: &[u8] = &[0xff, 0xff, 0xff, 0xff];
        let e = read(&mut huge).await.unwrap_err();
        assert!(e.to_string().contains("4294967295 bytes (0 came)"), "{e}");
    }
}
