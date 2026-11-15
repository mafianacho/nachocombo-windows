//! RFC 6455 framing, platform independent. Used by the plain-socket transport of the dev CLI and the tests
//! ([`super::socket_transport::SocketTransport`]); the Windows app itself speaks WebSocket through WinHTTP (TLS, system
//! proxy). Port of `PetWSFraming.swift`.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WSMessage {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close(Option<u16>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WSFramingError { MaskedServerFrame, FrameTooLarge, ReservedBits, BadOpcode, BadUTF8, FragmentedControl }

impl std::fmt::Display for WSFramingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{self:?}") }
}

pub struct WSEncoder;

impl WSEncoder {
    /// Client frames are always masked. `mask_key` must be 4 random bytes (injected for deterministic tests).
    pub fn client_frame(opcode: u8, payload: &[u8], mask_key: [u8; 4]) -> Vec<u8> {
        let n = payload.len();
        let mut out = Vec::with_capacity(n + 14);
        out.push(0x80 | (opcode & 0x0f));
        if n < 126 {
            out.push(0x80 | n as u8);
        } else if n < 65536 {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        } else {
            out.push(0x80 | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
        out.extend_from_slice(&mask_key);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask_key[i % 4]));
        out
    }

    pub fn text(s: &str, mask_key: [u8; 4]) -> Vec<u8> { Self::client_frame(1, s.as_bytes(), mask_key) }
    pub fn pong(d: &[u8], mask_key: [u8; 4]) -> Vec<u8> { Self::client_frame(10, d, mask_key) }
    /// 1000 normal closure.
    pub fn close(mask_key: [u8; 4]) -> Vec<u8> { Self::client_frame(8, &[0x03, 0xe8], mask_key) }
}

/// Incremental decoder for server → client frames (unmasked, possibly fragmented, possibly split across reads).
#[derive(Default)]
pub struct WSDecoder {
    buffer: Vec<u8>,
    fragments: Vec<u8>,
    fragment_opcode: Option<u8>,
}

impl WSDecoder {
    pub const MAX_PAYLOAD: usize = 1_000_000;

    pub fn new() -> Self { Self::default() }

    /// Every message the bytes so far complete. After an error the connection must be dropped.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<WSMessage>, WSFramingError> {
        self.buffer.extend_from_slice(chunk);
        let mut out = vec![];
        while let Some(m) = self.next()? {
            if let Some(m) = m { out.push(m); }
        }
        Ok(out)
    }

    /// None when more bytes are needed; `Some(None)` for a consumed continuation fragment.
    fn next(&mut self) -> Result<Option<Option<WSMessage>>, WSFramingError> {
        let b = &self.buffer;
        if b.len() < 2 { return Ok(None); }
        let fin = b[0] & 0x80 != 0;
        if b[0] & 0x70 != 0 { return Err(WSFramingError::ReservedBits); }
        let opcode = b[0] & 0x0f;
        if b[1] & 0x80 != 0 { return Err(WSFramingError::MaskedServerFrame); }
        let mut len = (b[1] & 0x7f) as usize;
        let mut off = 2;
        if len == 126 {
            if b.len() < 4 { return Ok(None); }
            len = u16::from_be_bytes([b[2], b[3]]) as usize;
            off = 4;
        } else if len == 127 {
            if b.len() < 10 { return Ok(None); }
            let v = u64::from_be_bytes([b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]]);
            if v > Self::MAX_PAYLOAD as u64 { return Err(WSFramingError::FrameTooLarge); }
            len = v as usize;
            off = 10;
        }
        if len > Self::MAX_PAYLOAD { return Err(WSFramingError::FrameTooLarge); }
        if b.len() < off + len { return Ok(None); }
        let payload: Vec<u8> = self.buffer[off..off + len].to_vec();
        self.buffer.drain(..off + len);

        match opcode {
            0x8..=0xA if !fin => Err(WSFramingError::FragmentedControl),
            0x8 => {
                let code = (payload.len() >= 2).then(|| u16::from_be_bytes([payload[0], payload[1]]));
                Ok(Some(Some(WSMessage::Close(code))))
            }
            0x9 => Ok(Some(Some(WSMessage::Ping(payload)))),
            0xA => Ok(Some(Some(WSMessage::Pong(payload)))),
            0x0..=0x2 => {
                if opcode != 0 {
                    self.fragment_opcode = Some(opcode);
                    self.fragments.clear();
                } else if self.fragment_opcode.is_none() {
                    return Err(WSFramingError::BadOpcode);
                }
                self.fragments.extend_from_slice(&payload);
                if self.fragments.len() > Self::MAX_PAYLOAD { return Err(WSFramingError::FrameTooLarge); }
                if !fin { return Ok(Some(None)); }
                let op = self.fragment_opcode.take().unwrap_or(1);
                let data = std::mem::take(&mut self.fragments);
                if op == 0x1 {
                    return String::from_utf8(data).map(|s| Some(Some(WSMessage::Text(s)))).map_err(|_| WSFramingError::BadUTF8);
                }
                Ok(Some(Some(WSMessage::Binary(data))))
            }
            _ => Err(WSFramingError::BadOpcode),
        }
    }
}

#[cfg(test)]
mod tests {
    //! `PetWSFramingTests` of `PetChatTests.swift`.
    use super::*;

    fn server_frame(opcode: u8, payload: &[u8], fin: bool) -> Vec<u8> {
        let mut d = vec![(if fin { 0x80 } else { 0 }) | opcode];
        if payload.len() < 126 {
            d.push(payload.len() as u8);
        } else {
            d.push(126);
            d.push((payload.len() >> 8) as u8);
            d.push((payload.len() & 0xff) as u8);
        }
        d.extend_from_slice(payload);
        d
    }

    #[test]
    fn client_frame_is_masked_and_round_trips() {
        let key = [1, 2, 3, 4];
        let f = WSEncoder::text("안녕", key);
        assert_eq!(f[0], 0x81);
        assert_eq!(f[1] & 0x80, 0x80);
        assert_eq!(f[2..6], key);
        let unmasked: Vec<u8> = (0..f.len() - 6).map(|i| f[6 + i] ^ key[i % 4]).collect();
        assert_eq!(String::from_utf8(unmasked).unwrap(), "안녕");
    }

    #[test]
    fn large_client_frame_uses_16_bit_length() {
        let f = WSEncoder::client_frame(1, &[0x61; 300], [0, 0, 0, 0]);
        assert_eq!(f[1], 0x80 | 126);
        assert_eq!((f[2] as usize) << 8 | f[3] as usize, 300);
        // and the 64-bit form above 65535
        let big = WSEncoder::client_frame(2, &vec![0; 70_000], [0, 0, 0, 0]);
        assert_eq!(big[1], 0x80 | 127);
        assert_eq!(u64::from_be_bytes(big[2..10].try_into().unwrap()), 70_000);
    }

    #[test]
    fn decodes_split_and_multiple_frames() {
        let mut d = WSDecoder::new();
        let mut all = server_frame(1, b"hello", true);
        all.extend(server_frame(9, &[7], true));
        assert_eq!(d.feed(&all[..3]), Ok(vec![]));
        assert_eq!(d.feed(&all[3..]), Ok(vec![WSMessage::Text("hello".into()), WSMessage::Ping(vec![7])]));
    }

    #[test]
    fn fragmented_text_message() {
        let mut d = WSDecoder::new();
        let mut m = server_frame(1, b"ab", false);
        m.extend(server_frame(0, b"cd", true));
        assert_eq!(d.feed(&m), Ok(vec![WSMessage::Text("abcd".into())]));
    }

    #[test]
    fn rejects_bad_frames() {
        use WSFramingError::*;
        assert_eq!(WSDecoder::new().feed(&[0x81, 0x80, 0, 0, 0, 0]), Err(MaskedServerFrame));
        assert_eq!(WSDecoder::new().feed(&[0xC1, 0x00]), Err(ReservedBits));
        assert_eq!(WSDecoder::new().feed(&[0x83, 0x00]), Err(BadOpcode));
        assert_eq!(WSDecoder::new().feed(&[0x81, 127, 0, 0, 0, 0, 0, 0x7f, 0xff, 0xff]), Err(FrameTooLarge));
        assert_eq!(WSDecoder::new().feed(&server_frame(1, &[0xff, 0xfe], true)), Err(BadUTF8));
        assert_eq!(WSDecoder::new().feed(&server_frame(0, &[1], true)), Err(BadOpcode)); // continuation without start
        assert_eq!(WSDecoder::new().feed(&server_frame(9, &[], false)), Err(FragmentedControl));
    }

    #[test]
    fn close_code() {
        let mut d = WSDecoder::new();
        assert_eq!(d.feed(&server_frame(8, &[0x03, 0xe8], true)), Ok(vec![WSMessage::Close(Some(1000))]));
        assert_eq!(d.feed(&server_frame(8, &[], true)), Ok(vec![WSMessage::Close(None)]));
    }
}
