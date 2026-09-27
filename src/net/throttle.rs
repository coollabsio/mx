//! Bandwidth limiting for `--limit-upload` / `--limit-download`.
//!
//! A [`Limiter`] is shared by all requests of the process (like mc's global limiter). Bodies
//! are wrapped with [`throttle_body`], which paces data frames through the limiter.

use aws_smithy_types::body::SdkBody;
use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, ready};
use std::time::{Duration, Instant};

/// Largest chunk released at once, so big in-memory bodies are paced smoothly.
const MAX_CHUNK: usize = 32 * 1024;

/// Byte-rate limiter: every chunk reserves `len / rate` seconds on a shared timeline.
#[derive(Debug)]
pub struct Limiter {
    bytes_per_sec: u64,
    next_free: Mutex<Option<Instant>>,
}

impl Limiter {
    pub fn new(bytes_per_sec: u64) -> Arc<Self> {
        Arc::new(Self {
            bytes_per_sec: bytes_per_sec.max(1),
            next_free: Mutex::new(None),
        })
    }

    /// Reserves `len` bytes; returns how long the caller must wait before sending them.
    pub fn reserve(&self, len: usize) -> Duration {
        let now = Instant::now();
        let mut next_free = self.next_free.lock().expect("limiter lock poisoned");
        let start = next_free.map_or(now, |next| next.max(now));
        let cost = Duration::from_secs_f64(len as f64 / self.bytes_per_sec as f64);
        *next_free = Some(start + cost);
        start - now
    }

    pub fn chunk_size(&self) -> usize {
        // About ten chunks per second keeps pacing smooth at low rates.
        ((self.bytes_per_sec / 10) as usize).clamp(1, MAX_CHUNK)
    }
}

/// Wraps `body` so its data is released at the limiter's rate. Retryable bodies stay retryable.
pub fn throttle_body(body: SdkBody, limiter: Arc<Limiter>) -> SdkBody {
    body.map(move |inner| {
        SdkBody::from_body_1_x(ThrottledBody {
            inner,
            limiter: limiter.clone(),
            pending: Bytes::new(),
            sleep: None,
        })
    })
}

struct ThrottledBody {
    inner: SdkBody,
    limiter: Arc<Limiter>,
    /// Data read from `inner` but not yet released.
    pending: Bytes,
    sleep: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl Body for ThrottledBody {
    type Data = Bytes;
    type Error = aws_smithy_types::body::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let this = self.get_mut();
        loop {
            if let Some(sleep) = this.sleep.as_mut() {
                ready!(sleep.as_mut().poll(cx));
                this.sleep = None;
                let len = this.pending.len().min(this.limiter.chunk_size());
                return Poll::Ready(Some(Ok(Frame::data(this.pending.split_to(len)))));
            }
            if !this.pending.is_empty() {
                let len = this.pending.len().min(this.limiter.chunk_size());
                let wait = this.limiter.reserve(len);
                this.sleep = Some(Box::pin(tokio::time::sleep(wait)));
                continue;
            }
            match ready!(Pin::new(&mut this.inner).poll_frame(cx)) {
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(data) => this.pending = data,
                    Err(frame) => return Poll::Ready(Some(Ok(frame))),
                },
                other => return Poll::Ready(other),
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.pending.is_empty() && self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        let inner = Body::size_hint(&self.inner);
        let pending = self.pending.len() as u64;
        let mut hint = SizeHint::new();
        hint.set_lower(inner.lower() + pending);
        if let Some(upper) = inner.upper() {
            hint.set_upper(upper + pending);
        }
        hint
    }
}

#[cfg(test)]
mod tests {
    use super::{Limiter, throttle_body};
    use aws_smithy_types::body::SdkBody;
    use http_body::Body;
    use std::time::{Duration, Instant};

    #[test]
    fn limiter_spaces_reservations() {
        let limiter = Limiter::new(1000);
        assert_eq!(limiter.reserve(500), Duration::ZERO);
        let wait = limiter.reserve(500);
        assert!(wait > Duration::from_millis(400) && wait <= Duration::from_millis(500));
    }

    #[test]
    fn throttled_body_keeps_content_and_paces_it() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let data = vec![7u8; 4000];
        let body = throttle_body(SdkBody::from(data.clone()), Limiter::new(8000));
        assert_eq!(Body::size_hint(&body).exact(), Some(4000));
        let started = Instant::now();
        let mut out = Vec::new();
        runtime.block_on(async {
            let mut body = std::pin::pin!(body);
            while let Some(frame) = std::future::poll_fn(|cx| body.as_mut().poll_frame(cx)).await {
                out.extend_from_slice(&frame.unwrap().into_data().unwrap());
            }
        });
        assert_eq!(out, data);
        // 4000 bytes at 8000 B/s in 800-byte chunks: the last one starts after 400ms.
        assert!(started.elapsed() >= Duration::from_millis(350));
        // Retryable bodies stay retryable.
        let body = throttle_body(SdkBody::from("abc"), Limiter::new(10));
        assert!(body.try_clone().is_some());
    }
}
