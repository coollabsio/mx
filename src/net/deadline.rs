//! mc's hidden `--conn-read-deadline` / `--conn-write-deadline` (mc `deadlineconn`): every
//! socket read or write must complete within the deadline, which restarts with each call.
//! [`DeadlineConnector`] wraps the TCP connector of the custom HTTP stack in `tls.rs`, below
//! TLS like Go's `net.Conn` wrapper.

use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::TokioIo;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::time::Sleep;

/// Per-read and per-write deadlines (`None` = no deadline).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Deadlines {
    pub read: Option<Duration>,
    pub write: Option<Duration>,
}

impl Deadlines {
    pub fn is_set(&self) -> bool {
        self.read.is_some() || self.write.is_some()
    }
}

static DEADLINES: OnceLock<Deadlines> = OnceLock::new();

/// Stores the process-wide deadlines (0 = no deadline). The first call wins.
pub fn set(read: Option<Duration>, write: Option<Duration>) {
    let positive = |value: Option<Duration>| value.filter(|d| !d.is_zero());
    let _ = DEADLINES.set(Deadlines {
        read: positive(read),
        write: positive(write),
    });
}

/// The configured deadlines (none before [`set`]).
pub fn get() -> Deadlines {
    DEADLINES.get().copied().unwrap_or_default()
}

/// IO wrapper with Go `SetReadDeadline`/`SetWriteDeadline` semantics per call: each read or
/// write gets a deadline of `now + d` when it starts and fails with a `TimedOut` error
/// (`read tcp LOCAL->REMOTE: i/o timeout`) once it passes, even if data is ready.
#[derive(Debug)]
pub struct DeadlineIo<T> {
    inner: T,
    deadlines: Deadlines,
    /// `LOCAL->REMOTE` for error messages.
    addrs: String,
    read: Option<Timer>,
    write: Option<Timer>,
}

#[derive(Debug)]
struct Timer {
    at: Instant,
    /// Created once the call goes pending.
    sleep: Option<Pin<Box<Sleep>>>,
}

impl<T> DeadlineIo<T> {
    pub fn new(inner: T, deadlines: Deadlines, addrs: String) -> Self {
        Self {
            inner,
            deadlines,
            addrs,
            read: None,
            write: None,
        }
    }
}

/// Runs one poll of an IO call under its deadline: the timer starts with the call, is checked
/// before and while the call is pending, and is cleared when the call completes.
fn with_deadline<R>(
    timer: &mut Option<Timer>,
    deadline: Option<Duration>,
    cx: &mut Context<'_>,
    timeout: impl FnOnce() -> io::Error,
    poll: impl FnOnce(&mut Context<'_>) -> Poll<io::Result<R>>,
) -> Poll<io::Result<R>> {
    let Some(deadline) = deadline else {
        return poll(cx);
    };
    let timer = timer.get_or_insert_with(|| Timer {
        at: Instant::now() + deadline,
        sleep: None,
    });
    let expired = |timer: &Timer| Instant::now() >= timer.at;
    if expired(timer) {
        return Poll::Ready(Err(timeout()));
    }
    match poll(cx) {
        Poll::Ready(result) => Poll::Ready(result),
        Poll::Pending => {
            let at = timer.at.into();
            let sleep = timer
                .sleep
                .get_or_insert_with(|| Box::pin(tokio::time::sleep_until(at)));
            if sleep.as_mut().poll(cx).is_ready() || expired(timer) {
                return Poll::Ready(Err(timeout()));
            }
            Poll::Pending
        }
    }
}

impl<T> DeadlineIo<T> {
    fn timeout(&self, op: &str) -> io::Error {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("{op} tcp {}: i/o timeout", self.addrs),
        )
    }
}

/// Clears `timer` once the call finished (successfully or not).
fn finish<R>(timer: &mut Option<Timer>, poll: Poll<io::Result<R>>) -> Poll<io::Result<R>> {
    if poll.is_ready() {
        *timer = None;
    }
    poll
}

impl<T: Read + Unpin> Read for DeadlineIo<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let error = this.timeout("read");
        let inner = &mut this.inner;
        let poll = with_deadline(
            &mut this.read,
            this.deadlines.read,
            cx,
            || error,
            |cx| Pin::new(inner).poll_read(cx, buf),
        );
        finish(&mut this.read, poll)
    }
}

impl<T: Write + Unpin> DeadlineIo<T> {
    fn poll_timed_write<R>(
        &mut self,
        cx: &mut Context<'_>,
        call: impl FnOnce(Pin<&mut T>, &mut Context<'_>) -> Poll<io::Result<R>>,
    ) -> Poll<io::Result<R>> {
        let error = self.timeout("write");
        let inner = &mut self.inner;
        let poll = with_deadline(
            &mut self.write,
            self.deadlines.write,
            cx,
            || error,
            |cx| call(Pin::new(inner), cx),
        );
        finish(&mut self.write, poll)
    }
}

impl<T: Write + Unpin> Write for DeadlineIo<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.get_mut()
            .poll_timed_write(cx, |inner, cx| inner.poll_write(cx, buf))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.get_mut()
            .poll_timed_write(cx, |inner, cx| inner.poll_write_vectored(cx, bufs))
    }
}

impl<T: Connection> Connection for DeadlineIo<T> {
    fn connected(&self) -> Connected {
        self.inner.connected()
    }
}

/// Connector (tower service) wrapping every TCP connection of `inner` in [`DeadlineIo`]
/// (only when deadlines are set).
#[derive(Debug, Clone)]
pub struct DeadlineConnector<C> {
    inner: C,
    deadlines: Deadlines,
}

impl<C> DeadlineConnector<C> {
    pub fn new(inner: C, deadlines: Deadlines) -> Self {
        Self { inner, deadlines }
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type TcpIo = TokioIo<TcpStream>;

impl<C> tower_service::Service<http::Uri> for DeadlineConnector<C>
where
    C: tower_service::Service<http::Uri, Response = TcpIo> + Send,
    C::Error: Into<BoxError>,
    C::Future: Send + 'static,
{
    type Response = DeadlineIo<TcpIo>;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, BoxError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, uri: http::Uri) -> Self::Future {
        let connecting = self.inner.call(uri);
        let deadlines = self.deadlines;
        Box::pin(async move {
            let io = connecting.await.map_err(Into::into)?;
            let addr = |addr: io::Result<std::net::SocketAddr>| {
                addr.map(|a| a.to_string()).unwrap_or_default()
            };
            let addrs = format!(
                "{}->{}",
                addr(io.inner().local_addr()),
                addr(io.inner().peer_addr())
            );
            Ok(DeadlineIo::new(io, deadlines, addrs))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper_util::client::legacy::connect::HttpConnector;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn deadlines(read: u64, write: u64) -> Deadlines {
        Deadlines {
            read: Some(Duration::from_millis(read)),
            write: Some(Duration::from_millis(write)),
        }
    }

    async fn read_some<T: Read + Unpin>(io: &mut TokioIo<T>) -> io::Result<Vec<u8>>
    where
        TokioIo<T>: tokio::io::AsyncRead,
    {
        let mut buf = vec![0u8; 64];
        let n = io.read(&mut buf).await?;
        buf.truncate(n);
        Ok(buf)
    }

    /// A slow server: each chunk arrives within the deadline, so the deadline restarts per
    /// read; a silent server fails the read after the deadline.
    #[tokio::test]
    async fn read_deadline_restarts_per_read_and_times_out_when_idle() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            for _ in 0..3 {
                tokio::time::sleep(Duration::from_millis(150)).await;
                socket.write_all(b"x").await.unwrap();
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let mut connector = DeadlineConnector::new(HttpConnector::new(), deadlines(400, 400));
        let uri: http::Uri = format!("http://{addr}").parse().unwrap();
        let io = tower_service::Service::call(&mut connector, uri)
            .await
            .unwrap();
        let mut io = TokioIo::new(io);
        for _ in 0..3 {
            assert_eq!(read_some(&mut io).await.unwrap(), b"x");
        }
        let started = Instant::now();
        let err = read_some(&mut io).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        let text = err.to_string();
        assert!(
            text.starts_with("read tcp 127.0.0.1:") && text.ends_with(": i/o timeout"),
            "{text}"
        );
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(350) && waited < Duration::from_secs(3));
    }

    /// A server that never reads: writes fill the socket buffers, then the next write stays
    /// pending and fails after the write deadline.
    #[tokio::test]
    async fn write_deadline_fails_stalled_writes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(10)).await;
            drop(socket);
        });

        let mut connector = DeadlineConnector::new(HttpConnector::new(), deadlines(10_000, 300));
        let uri: http::Uri = format!("http://{addr}").parse().unwrap();
        let io = tower_service::Service::call(&mut connector, uri)
            .await
            .unwrap();
        let mut io = TokioIo::new(io);
        let chunk = vec![0u8; 1 << 20];
        let started = Instant::now();
        let err = loop {
            if let Err(err) = io.write_all(&chunk).await {
                break err;
            }
            assert!(
                started.elapsed() < Duration::from_secs(8),
                "write never stalled"
            );
        };
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        server.abort();
    }
}
