//! Prometheus metrics for `mirror --monitoring-address` (mc `mirror-main.go`): the same metric
//! names, served as text exposition format at `http://ADDRESS/metrics`. mc's Go runtime and
//! process collectors are not reproduced.

use anyhow::Result;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// mc `prometheus.ExponentialBuckets(1, 20, 5)` (milliseconds).
const BUCKETS: [f64; 5] = [1.0, 20.0, 400.0, 8000.0, 160000.0];

#[derive(Default)]
struct Histogram {
    counts: [u64; BUCKETS.len()],
    count: u64,
    sum: f64,
}

pub struct Metrics {
    total_ops: AtomicU64,
    uploaded_bytes: AtomicU64,
    failed_ops: AtomicU64,
    restarts: AtomicU64,
    durations: Mutex<BTreeMap<&'static str, Histogram>>,
}

pub static METRICS: Metrics = Metrics {
    total_ops: AtomicU64::new(0),
    uploaded_bytes: AtomicU64::new(0),
    failed_ops: AtomicU64::new(0),
    restarts: AtomicU64::new(0),
    durations: Mutex::new(BTreeMap::new()),
};

impl Metrics {
    /// One finished mirror operation (`mc_mirror_total_s3ops`); failures also count in
    /// `mc_mirror_failed_s3ops`.
    pub fn op(&self, success: bool) {
        self.total_ops.fetch_add(1, Ordering::Relaxed);
        if !success {
            self.failed_ops.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn uploaded(&self, bytes: i64) {
        self.uploaded_bytes
            .fetch_add(bytes.max(0) as u64, Ordering::Relaxed);
    }

    pub fn restart(&self) {
        self.restarts.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one successful object copy in `mc_mirror_replication_duration`.
    pub fn replicated(&self, size: i64, elapsed: Duration) {
        let millis = elapsed.as_millis() as f64;
        let mut durations = self.durations.lock().unwrap();
        let histogram = durations.entry(size_tag(size)).or_default();
        for (count, bound) in histogram.counts.iter_mut().zip(BUCKETS) {
            if millis <= bound {
                *count += 1;
            }
        }
        histogram.count += 1;
        histogram.sum += millis;
    }

    /// Prometheus text exposition format, metrics sorted by name like client_golang.
    pub fn render(&self) -> String {
        let counter = |out: &mut String, name: &str, help: &str, value: u64| {
            out.push_str(&format!(
                "# HELP {name} {help}\n# TYPE {name} counter\n{name} {value}\n"
            ));
        };
        let mut out = String::new();
        counter(
            &mut out,
            "mc_mirror_failed_s3ops",
            "The total number of failed mirror operations",
            self.failed_ops.load(Ordering::Relaxed),
        );
        let name = "mc_mirror_replication_duration";
        out.push_str(&format!(
            "# HELP {name} Histogram of replication time in ms per object sizes\n# TYPE {name} histogram\n"
        ));
        for (tag, histogram) in self.durations.lock().unwrap().iter() {
            for (count, bound) in histogram.counts.iter().zip(BUCKETS) {
                out.push_str(&format!(
                    "{name}_bucket{{object_size=\"{tag}\",le=\"{bound}\"}} {count}\n"
                ));
            }
            out.push_str(&format!(
                "{name}_bucket{{object_size=\"{tag}\",le=\"+Inf\"}} {}\n",
                histogram.count
            ));
            out.push_str(&format!(
                "{name}_sum{{object_size=\"{tag}\"}} {}\n",
                histogram.sum
            ));
            out.push_str(&format!(
                "{name}_count{{object_size=\"{tag}\"}} {}\n",
                histogram.count
            ));
        }
        counter(
            &mut out,
            "mc_mirror_total_restarts",
            "The number of mirror restarts",
            self.restarts.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "mc_mirror_total_s3ops",
            "The total number of mirror operations",
            self.total_ops.load(Ordering::Relaxed),
        );
        counter(
            &mut out,
            "mc_mirror_total_s3uploaded_bytes",
            "The total number of bytes uploaded",
            self.uploaded_bytes.load(Ordering::Relaxed),
        );
        out
    }
}

/// mc `convertSizeToTag`.
fn size_tag(size: i64) -> &'static str {
    const KIB: i64 = 1024;
    match size {
        s if s < KIB => "LESS_THAN_1_KiB",
        s if s < KIB * KIB => "LESS_THAN_1_MiB",
        s if s < 10 * KIB * KIB => "LESS_THAN_10_MiB",
        s if s < 100 * KIB * KIB => "LESS_THAN_100_MiB",
        s if s < KIB * KIB * KIB => "LESS_THAN_1_GiB",
        _ => "GREATER_THAN_1_GiB",
    }
}

/// Binds `address` (`HOST:PORT`) and serves `GET /metrics` in the background.
pub async fn serve(address: &str) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(address).await?;
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(respond(stream));
        }
    });
    Ok(())
}

/// Answers one HTTP/1.x request and closes the connection.
async fn respond(mut stream: tokio::net::TcpStream) {
    let mut request = Vec::new();
    let mut buf = [0u8; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < 16 * 1024 {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => request.extend_from_slice(&buf[..n]),
        }
    }
    let line = String::from_utf8_lossy(&request);
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let path = path.split('?').next().unwrap_or("");
    let (status, body) = match (method, path) {
        ("GET" | "HEAD", "/metrics") => ("200 OK", METRICS.render()),
        _ => ("404 Not Found", "404 page not found\n".to_string()),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; version=0.0.4; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    if method != "HEAD" {
        let _ = stream.write_all(body.as_bytes()).await;
    }
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_sizes_like_mc() {
        assert_eq!(size_tag(0), "LESS_THAN_1_KiB");
        assert_eq!(size_tag(1024), "LESS_THAN_1_MiB");
        assert_eq!(size_tag(5 << 20), "LESS_THAN_10_MiB");
        assert_eq!(size_tag(50 << 20), "LESS_THAN_100_MiB");
        assert_eq!(size_tag(500 << 20), "LESS_THAN_1_GiB");
        assert_eq!(size_tag(2 << 30), "GREATER_THAN_1_GiB");
    }

    #[test]
    fn renders_prometheus_text() {
        let metrics = Metrics {
            total_ops: AtomicU64::new(3),
            uploaded_bytes: AtomicU64::new(12),
            failed_ops: AtomicU64::new(1),
            restarts: AtomicU64::new(0),
            durations: Mutex::new(BTreeMap::new()),
        };
        metrics.replicated(4, Duration::from_millis(15));
        let text = metrics.render();
        assert!(text.contains("# TYPE mc_mirror_total_s3ops counter\nmc_mirror_total_s3ops 3\n"));
        assert!(text.contains("mc_mirror_total_s3uploaded_bytes 12\n"));
        assert!(text.contains("mc_mirror_failed_s3ops 1\n"));
        assert!(text.contains(
            "mc_mirror_replication_duration_bucket{object_size=\"LESS_THAN_1_KiB\",le=\"1\"} 0\n"
        ));
        assert!(text.contains(
            "mc_mirror_replication_duration_bucket{object_size=\"LESS_THAN_1_KiB\",le=\"20\"} 1\n"
        ));
        assert!(
            text.contains(
                "mc_mirror_replication_duration_count{object_size=\"LESS_THAN_1_KiB\"} 1\n"
            )
        );
    }
}
