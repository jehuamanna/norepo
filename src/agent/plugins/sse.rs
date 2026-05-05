//! Hand-rolled SSE (Server-Sent Events) parser.
//!
//! Consumes a `Stream<Item = Result<Bytes>>` and yields parsed `SseEvent`s.
//! Buffers across reads so partial frames are reassembled.

use bytes::{Bytes, BytesMut};
use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
    pub id: Option<String>,
}

/// Wraps an async byte stream and exposes parsed SSE events.
pub struct SseStream<S> {
    inner: S,
    buf: BytesMut,
    cur_event: String,
    cur_data: String,
    cur_id: Option<String>,
    done: bool,
}

impl<S> SseStream<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            buf: BytesMut::with_capacity(8192),
            cur_event: String::new(),
            cur_data: String::new(),
            cur_id: None,
            done: false,
        }
    }
}

impl<S, E> Stream for SseStream<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    type Item = Result<SseEvent, String>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let me = self.get_mut();
        loop {
            // Try to extract a complete event from the buffer first.
            if let Some(ev) = take_event(me) {
                return Poll::Ready(Some(Ok(ev)));
            }
            if me.done {
                return Poll::Ready(None);
            }
            match Pin::new(&mut me.inner).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    me.done = true;
                    if let Some(ev) = take_event(me) {
                        return Poll::Ready(Some(Ok(ev)));
                    }
                    return Poll::Ready(None);
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(format!("sse upstream: {e}"))))
                }
                Poll::Ready(Some(Ok(chunk))) => {
                    me.buf.extend_from_slice(&chunk);
                }
            }
        }
    }
}

fn take_event<S>(s: &mut SseStream<S>) -> Option<SseEvent> {
    // Find a `\n\n` (event boundary).
    let needle = b"\n\n";
    let pos = s.buf.windows(2).position(|w| w == needle)?;
    let frame = s.buf.split_to(pos + 2);
    let frame_str = std::str::from_utf8(&frame).unwrap_or("").to_string();
    parse_frame(&frame_str, s)
}

fn parse_frame<S>(frame: &str, s: &mut SseStream<S>) -> Option<SseEvent> {
    let mut event = std::mem::take(&mut s.cur_event);
    let mut data = std::mem::take(&mut s.cur_data);
    let mut id = s.cur_id.take();
    let mut had_any = false;
    for line in frame.lines() {
        if line.is_empty() {
            continue;
        }
        had_any = true;
        if let Some(rest) = line.strip_prefix("event:") {
            event = rest.trim_start().to_string();
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        } else if let Some(rest) = line.strip_prefix("id:") {
            id = Some(rest.trim_start().to_string());
        }
        // Ignore unknown fields per SSE spec.
    }
    if !had_any {
        return None;
    }
    Some(SseEvent { event, data, id })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures::stream::{self, StreamExt};

    type TestErr = std::convert::Infallible;

    async fn collect(input: Vec<&'static [u8]>) -> Vec<Result<SseEvent, String>> {
        let s = stream::iter(
            input
                .into_iter()
                .map(|b| Ok::<_, TestErr>(Bytes::from_static(b))),
        );
        SseStream::new(s).collect().await
    }

    #[tokio::test]
    async fn basic_event() {
        let r = collect(vec![b"event: foo\ndata: hi\n\n"]).await;
        assert_eq!(r.len(), 1);
        let e = r[0].as_ref().unwrap();
        assert_eq!(e.event, "foo");
        assert_eq!(e.data, "hi");
    }

    #[tokio::test]
    async fn byte_by_byte() {
        let frame = b"event: foo\ndata: hi\n\n";
        let chunks: Vec<&'static [u8]> = (0..frame.len())
            .map(|i| &frame[i..i + 1])
            .collect();
        let r = collect(chunks).await;
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].as_ref().unwrap().data, "hi");
    }

    #[tokio::test]
    async fn multiline_data() {
        let r = collect(vec![b"event: msg\ndata: line1\ndata: line2\n\n"]).await;
        let e = r[0].as_ref().unwrap();
        assert_eq!(e.data, "line1\nline2");
    }

    #[tokio::test]
    async fn multiple_events() {
        let r = collect(vec![b"event: a\ndata: 1\n\nevent: b\ndata: 2\n\n"]).await;
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].as_ref().unwrap().event, "a");
        assert_eq!(r[1].as_ref().unwrap().event, "b");
    }
}
