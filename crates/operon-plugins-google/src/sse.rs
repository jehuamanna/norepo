//! Hand-rolled SSE parser for Gemini's `:streamGenerateContent?alt=sse` endpoint.

use bytes::{Bytes, BytesMut};
use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    pub data: String,
}

pub struct SseStream<S> {
    inner: S,
    buf: BytesMut,
    done: bool,
}

impl<S> SseStream<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            buf: BytesMut::with_capacity(8192),
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
                Poll::Ready(Some(Ok(chunk))) => me.buf.extend_from_slice(&chunk),
            }
        }
    }
}

fn take_event<S>(s: &mut SseStream<S>) -> Option<SseEvent> {
    let needle = b"\n\n";
    let pos = s.buf.windows(2).position(|w| w == needle)?;
    let frame = s.buf.split_to(pos + 2);
    let frame_str = std::str::from_utf8(&frame).unwrap_or("").to_string();
    let mut data = String::new();
    let mut had_any = false;
    for line in frame_str.lines() {
        if line.is_empty() {
            continue;
        }
        had_any = true;
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        }
    }
    if !had_any {
        return None;
    }
    Some(SseEvent { data })
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
    async fn parses_data_frame() {
        let r = collect(vec![b"data: {\"x\":1}\n\n"]).await;
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].as_ref().unwrap().data, "{\"x\":1}");
    }
}
