//! LSP 帧编解码：`Content-Length` 头 + JSON 体。

use std::io::{BufRead, Write};

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("非法帧头")]
    MalformedHeader,
    #[error("消息体超长（>{0} 字节）")]
    TooLarge(usize),
}

const MAX_MESSAGE: usize = 64 * 1024 * 1024;

/// 编码为 LSP 帧。
pub fn encode(body: &str) -> Vec<u8> {
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
}

/// 读取一帧（阻塞）。
pub fn read_message<R: BufRead>(reader: &mut R) -> Result<String, CodecError> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Err(CodecError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "stream ended",
            )));
        }
        let line = line.trim_end();
        if line.is_empty() {
            break; // 头部结束
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = Some(
                    v.trim()
                        .parse::<usize>()
                        .map_err(|_| CodecError::MalformedHeader)?,
                );
            }
        }
    }
    let len = content_length.ok_or(CodecError::MalformedHeader)?;
    if len > MAX_MESSAGE {
        return Err(CodecError::TooLarge(len));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    String::from_utf8(buf).map_err(|_| CodecError::Io(std::io::Error::other("invalid utf-8")))
}

/// 写一帧（阻塞）。
pub fn write_message<W: Write>(writer: &mut W, body: &str) -> Result<(), CodecError> {
    writer.write_all(&encode(body))?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    #[test]
    fn roundtrip() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let framed = encode(body);
        let mut reader = BufReader::new(&framed[..]);
        let got = read_message(&mut reader).unwrap();
        assert_eq!(got, body);
    }

    #[test]
    fn multiple_frames_in_sequence() {
        let mut buf = Vec::new();
        write_message(&mut buf, "one").unwrap();
        write_message(&mut buf, "two-two").unwrap();
        let mut reader = BufReader::new(&buf[..]);
        assert_eq!(read_message(&mut reader).unwrap(), "one");
        assert_eq!(read_message(&mut reader).unwrap(), "two-two");
    }

    #[test]
    fn missing_content_length_rejected() {
        let data = b"X-Nothing: 1\r\n\r\nbody";
        let mut reader = BufReader::new(&data[..]);
        assert!(read_message(&mut reader).is_err());
    }

    #[test]
    fn oversized_rejected() {
        let data = b"Content-Length: 999999999999\r\n\r\nx";
        let mut reader = BufReader::new(&data[..]);
        assert!(matches!(
            read_message(&mut reader),
            Err(CodecError::TooLarge(_))
        ));
    }
}
