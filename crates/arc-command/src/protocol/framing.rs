//! Newline-delimited framing with bounded memory, and an in-memory byte pipe.

use std::collections::VecDeque;
use std::io::{self, BufRead, ErrorKind, Read, Write};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

/// One frame of the input stream.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    /// A line, without its newline, is in the buffer.
    Line,
    /// A line longer than the limit was consumed and dropped.
    TooLong,
    /// The stream ended.
    End,
}

/// Reads the next line of `input` into `buf` (cleared first). A line longer than `max` bytes is
/// consumed up to its newline without being kept, so memory stays bounded and the stream stays
/// synchronized. A last line without a newline counts as a line.
pub(crate) fn next_frame<R: BufRead>(
    input: &mut R,
    max: usize,
    buf: &mut Vec<u8>,
) -> io::Result<Frame> {
    buf.clear();
    let (mut too_long, mut any) = (false, false);
    loop {
        let chunk = match input.fill_buf() {
            Ok(chunk) => chunk,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if chunk.is_empty() {
            return Ok(match (too_long, any) {
                (true, _) => Frame::TooLong,
                (false, true) => Frame::Line,
                (false, false) => Frame::End,
            });
        }
        any = true;
        let newline = chunk.iter().position(|&b| b == b'\n');
        let (part, used) = match newline {
            Some(at) => (chunk.get(..at).unwrap_or_default(), at + 1),
            None => (chunk, chunk.len()),
        };
        if !too_long {
            if buf.len().saturating_add(part.len()) > max {
                too_long = true;
                buf.clear();
                buf.shrink_to_fit();
            } else {
                buf.extend_from_slice(part);
            }
        }
        input.consume(used);
        if newline.is_some() {
            return Ok(if too_long {
                Frame::TooLong
            } else {
                Frame::Line
            });
        }
    }
}

/// The writing end of an in-memory byte pipe. Dropping it ends the stream for the reader.
#[derive(Debug, Clone)]
pub struct PipeWriter(Sender<Vec<u8>>);

/// The reading end of an in-memory byte pipe; it implements [`BufRead`] and blocks until bytes
/// arrive, the writer is dropped (end of stream) or the timeout passes (`TimedOut`).
#[derive(Debug)]
pub struct PipeReader {
    from: Receiver<Vec<u8>>,
    chunk: VecDeque<u8>,
    timeout: Option<Duration>,
}

/// An in-memory byte pipe: what is written to the writer is read from the reader. Two pipes
/// make the duplex connection that tests drive a session over.
#[must_use]
pub fn pipe() -> (PipeWriter, PipeReader) {
    let (to, from) = channel();
    (
        PipeWriter(to),
        PipeReader {
            from,
            chunk: VecDeque::new(),
            timeout: None,
        },
    )
}

impl PipeReader {
    /// Makes reads fail with `TimedOut` after waiting `timeout` for bytes.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

impl Write for PipeWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .send(bytes.to_vec())
            .map_err(|_| io::Error::from(ErrorKind::BrokenPipe))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for PipeReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let n = available.len().min(out.len());
        if let (Some(to), Some(from)) = (out.get_mut(..n), available.get(..n)) {
            to.copy_from_slice(from);
        }
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for PipeReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        while self.chunk.is_empty() {
            let next = match self.timeout {
                Some(timeout) => self.from.recv_timeout(timeout).map_err(|e| match e {
                    RecvTimeoutError::Timeout => Some(io::Error::from(ErrorKind::TimedOut)),
                    RecvTimeoutError::Disconnected => None,
                }),
                None => self.from.recv().map_err(|_| None),
            };
            match next {
                Ok(bytes) => self.chunk.extend(bytes),
                Err(Some(error)) => return Err(error),
                Err(None) => return Ok(&[]),
            }
        }
        Ok(self.chunk.as_slices().0)
    }

    fn consume(&mut self, n: usize) {
        self.chunk.drain(..n.min(self.chunk.len()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(input: &[u8], max: usize) -> Vec<(Frame, Vec<u8>)> {
        let mut reader = io::BufReader::with_capacity(3, input);
        let mut buf = Vec::new();
        let mut out = Vec::new();
        loop {
            let frame = next_frame(&mut reader, max, &mut buf).unwrap();
            if frame == Frame::End {
                return out;
            }
            out.push((frame, buf.clone()));
        }
    }

    #[test]
    fn long_lines_are_skipped_and_the_next_line_is_read() {
        let got = frames(b"ab\n0123456789\ncd\nlast", 4);
        assert_eq!(
            got,
            vec![
                (Frame::Line, b"ab".to_vec()),
                (Frame::TooLong, vec![]),
                (Frame::Line, b"cd".to_vec()),
                (Frame::Line, b"last".to_vec()),
            ]
        );
        assert_eq!(frames(b"0123456789", 4), vec![(Frame::TooLong, vec![])]);
        assert_eq!(frames(b"\n", 4), vec![(Frame::Line, vec![])]);
    }

    #[test]
    fn pipe_carries_bytes_and_ends_when_the_writer_drops() {
        let (mut w, mut r) = pipe();
        w.write_all(b"hello\nwor").unwrap();
        w.write_all(b"ld\n").unwrap();
        drop(w);
        let mut text = String::new();
        r.read_to_string(&mut text).unwrap();
        assert_eq!(text, "hello\nworld\n");
        let (_w, r) = pipe();
        let mut r = r.with_timeout(Duration::from_millis(1));
        assert_eq!(r.fill_buf().unwrap_err().kind(), ErrorKind::TimedOut);
    }
}
