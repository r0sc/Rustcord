//! The pipe's framing: `op: u32 LE`, `len: u32 LE`, `len` bytes of JSON (docs/RPC_DESIGN.md
//! §2.2). Pure: `decode` takes what has been read so far, so a frame split across reads waits.

use serde_json::Value;

/// A rich presence is under 2 KiB; this keeps a bad client from making Rustcord buffer more.
pub const MAX_FRAME: usize = 64 * 1024;

pub const HANDSHAKE: u32 = 0;
pub const FRAME: u32 = 1;
pub const CLOSE: u32 = 2;
pub const PING: u32 = 3;
pub const PONG: u32 = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub op: u32,
    pub body: Vec<u8>,
}

impl Frame {
    pub fn json(op: u32, body: &Value) -> Frame {
        Frame {
            op,
            body: body.to_string().into_bytes(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + self.body.len());
        out.extend_from_slice(&self.op.to_le_bytes());
        out.extend_from_slice(&(self.body.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.body);
        out
    }
}

/// The next complete frame of `buf` (removed from it), `None` while more bytes are needed. An
/// unknown op or a length over `MAX_FRAME` is an error as soon as the header is in.
pub fn decode(buf: &mut Vec<u8>) -> Result<Option<Frame>, &'static str> {
    let Some(head) = buf.first_chunk::<8>() else {
        return Ok(None);
    };
    let op = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    if op > PONG {
        return Err("Unknown opcode");
    }
    if len > MAX_FRAME {
        return Err("Frame too large");
    }
    if buf.len() < 8 + len {
        return Ok(None);
    }
    let body = buf[8..8 + len].to_vec();
    buf.drain(..8 + len);
    Ok(Some(Frame { op, body }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(op: u32, body: &str) -> Frame {
        Frame {
            op,
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn round_trip_split_at_every_offset_and_two_in_one_buffer() {
        let (a, b) = (
            frame(HANDSHAKE, r#"{"v":1,"client_id":"1"}"#),
            frame(PING, "x"),
        );
        let bytes = [a.encode(), b.encode()].concat();
        for cut in 0..=bytes.len() {
            let mut buf = bytes[..cut].to_vec();
            let mut got = Vec::new();
            while let Some(f) = decode(&mut buf).unwrap() {
                got.push(f);
            }
            buf.extend_from_slice(&bytes[cut..]);
            while let Some(f) = decode(&mut buf).unwrap() {
                got.push(f);
            }
            assert_eq!(got, [a.clone(), b.clone()], "cut at {cut}");
            assert!(buf.is_empty());
        }
        // An empty body is a frame too.
        assert_eq!(
            decode(&mut frame(PONG, "").encode()),
            Ok(Some(frame(PONG, "")))
        );
    }

    #[test]
    fn oversize_and_unknown_ops_are_errors() {
        let header = |op: u32, len: usize| [op.to_le_bytes(), (len as u32).to_le_bytes()].concat();
        assert_eq!(
            decode(&mut header(FRAME, MAX_FRAME + 1)),
            Err("Frame too large")
        );
        let mut full = header(FRAME, MAX_FRAME);
        full.extend(std::iter::repeat_n(b' ', MAX_FRAME));
        assert_eq!(
            decode(&mut full).map(|f| f.map(|f| f.body.len())),
            Ok(Some(MAX_FRAME))
        );
        assert_eq!(decode(&mut header(5, 0)), Err("Unknown opcode"));
        assert_eq!(decode(&mut header(u32::MAX, 3)), Err("Unknown opcode"));
        assert_eq!(decode(&mut vec![1, 0, 0]), Ok(None), "a short header waits");
    }
}
