//! Transfer progress for `cp`/`mv`: a one-line bar redrawn with `\r` on a terminal, or plain
//! byte accounting with an mc-style summary (`Total | Transferred | Duration | Speed`).

use serde::Serialize;
use std::io::Write;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, ReadBuf};

const REFRESH: Duration = Duration::from_millis(125);
const CAPTION_WIDTH: usize = 32;
const BAR_WIDTH: usize = 20;

pub struct Progress {
    transferred: AtomicU64,
    total: AtomicU64,
    start: Instant,
    caption: Mutex<String>,
    bar: bool,
    stop: AtomicBool,
    drawer: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Progress {
    /// Creates a progress tracker. With `bar`, a background thread redraws the bar on stdout.
    pub fn new(total: u64, bar: bool) -> Arc<Self> {
        let progress = Arc::new(Self {
            transferred: AtomicU64::new(0),
            total: AtomicU64::new(total),
            start: Instant::now(),
            caption: Mutex::new(String::new()),
            bar,
            stop: AtomicBool::new(false),
            drawer: Mutex::new(None),
        });
        if bar {
            let shared = progress.clone();
            let handle = std::thread::spawn(move || {
                while !shared.stop.load(Ordering::Relaxed) {
                    shared.draw();
                    std::thread::sleep(REFRESH);
                }
            });
            *progress.drawer.lock().unwrap() = Some(handle);
        }
        progress
    }

    pub fn is_bar(&self) -> bool {
        self.bar
    }

    pub fn add(&self, bytes: u64) {
        self.transferred.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn transferred(&self) -> u64 {
        self.transferred.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    pub fn set_caption(&self, caption: &str) {
        *self.caption.lock().unwrap() = caption.to_string();
    }

    /// Clears the bar line so a message can be printed (no-op without a bar).
    pub fn erase_line(&self) {
        if self.bar {
            print!("\r\x1b[2K");
            let _ = std::io::stdout().flush();
        }
    }

    /// Stops redrawing. On success the final bar stays visible; otherwise it is erased.
    pub fn finish(&self, success: bool) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.drawer.lock().unwrap().take() {
            let _ = handle.join();
        }
        if self.bar {
            if success {
                self.draw();
                println!();
            } else {
                self.erase_line();
            }
        }
    }

    fn draw(&self) {
        let caption = self.caption.lock().unwrap().clone();
        let line = render_bar(
            &caption,
            self.transferred(),
            self.total(),
            self.start.elapsed(),
        );
        print!("\r{line}\x1b[K");
        let _ = std::io::stdout().flush();
    }

    /// Final accounting (mc `accountStat`).
    pub fn stat(&self) -> AccountStat {
        let transferred = self.transferred();
        let elapsed = self.start.elapsed();
        AccountStat {
            status: "success",
            total: self.total(),
            transferred,
            duration: elapsed.as_nanos() as u64,
            speed: speed(transferred, elapsed),
        }
    }
}

/// mc `accountStat` JSON: `duration` is in nanoseconds, `speed` in bytes per second.
#[derive(Debug, Clone, Serialize)]
pub struct AccountStat {
    pub status: &'static str,
    pub total: u64,
    pub transferred: u64,
    pub duration: u64,
    pub speed: f64,
}

impl AccountStat {
    /// Box table like mc prints after a quiet copy.
    pub fn table(&self) -> String {
        let header = ["Total", "Transferred", "Duration", "Speed"];
        let values = [
            format_bytes(self.total),
            format_bytes(self.transferred),
            format_duration(Duration::from_nanos(self.duration)),
            format!("{}/s", format_bytes(self.speed as u64)),
        ];
        let widths: Vec<usize> = header
            .iter()
            .zip(&values)
            .map(|(h, v)| h.chars().count().max(v.chars().count()))
            .collect();
        let rule = |left: &str, mid: &str, right: &str| {
            let cells: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
            format!("{left}{}{right}", cells.join(mid))
        };
        let row = |cells: Vec<String>| {
            let cells: Vec<String> = cells
                .iter()
                .zip(&widths)
                .map(|(c, w)| format!(" {c:<w$} "))
                .collect();
            format!("│{}│", cells.join("│"))
        };
        [
            rule("┌", "┬", "┐"),
            row(header.iter().map(|h| h.to_string()).collect()),
            row(values.to_vec()),
            rule("└", "┴", "┘"),
        ]
        .join("\n")
    }
}

fn speed(bytes: u64, elapsed: Duration) -> f64 {
    let secs = elapsed.as_secs_f64();
    if bytes == 0 || secs <= 0.0 {
        0.0
    } else {
        bytes as f64 / secs
    }
}

/// Renders one bar line: caption, bytes/total, bar, percent, speed.
pub fn render_bar(caption: &str, current: u64, total: u64, elapsed: Duration) -> String {
    let caption = fit_caption(caption, CAPTION_WIDTH);
    let ratio = if total == 0 {
        1.0
    } else {
        (current as f64 / total as f64).min(1.0)
    };
    let filled = (ratio * BAR_WIDTH as f64).round() as usize;
    format!(
        "{caption} {} / {} ┃{}{}┃ {:.2}% {}/s",
        format_bytes(current),
        format_bytes(total),
        "▓".repeat(filled),
        "░".repeat(BAR_WIDTH - filled),
        ratio * 100.0,
        format_bytes(speed(current, elapsed) as u64),
    )
}

/// Pads or front-truncates (`...tail`) a caption to `width` characters.
pub fn fit_caption(caption: &str, width: usize) -> String {
    let count = caption.chars().count();
    if count > width {
        let tail: String = caption.chars().skip(count - (width - 3)).collect();
        format!("...{tail}")
    } else {
        format!("{caption:<width$}")
    }
}

/// Byte sizes in the style of mc's progress output (`12 B`, `1.50 KiB`, `3.00 GiB`).
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [(&str, u64); 4] = [
        ("TiB", 1 << 40),
        ("GiB", 1 << 30),
        ("MiB", 1 << 20),
        ("KiB", 1 << 10),
    ];
    for (unit, size) in UNITS {
        if bytes >= size {
            return format!("{:.2} {unit}", bytes as f64 / size as f64);
        }
    }
    format!("{bytes} B")
}

/// `00m05s`, `1h02m03s`.
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let (hours, minutes, seconds) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if hours > 0 {
        format!("{hours}h{minutes:02}m{seconds:02}s")
    } else {
        format!("{minutes:02}m{seconds:02}s")
    }
}

/// Counts bytes read through it into a [`Progress`].
pub struct ProgressReader<R> {
    inner: R,
    progress: Arc<Progress>,
}

impl<R> ProgressReader<R> {
    pub fn new(inner: R, progress: Arc<Progress>) -> Self {
        Self { inner, progress }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for ProgressReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let result = Pin::new(&mut this.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &result {
            this.progress.add((buf.filled().len() - before) as u64);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes_and_durations() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1536), "1.50 KiB");
        assert_eq!(format_bytes(3 << 30), "3.00 GiB");
        assert_eq!(format_duration(Duration::from_secs(5)), "00m05s");
        assert_eq!(format_duration(Duration::from_secs(3723)), "1h02m03s");
    }

    #[test]
    fn fits_captions() {
        assert_eq!(fit_caption("abc", 5), "abc  ");
        assert_eq!(fit_caption("abcdefghij", 8), "...fghij");
    }

    #[test]
    fn renders_bar_and_table() {
        let line = render_bar("play/b/k:", 512, 1024, Duration::from_secs(1));
        assert!(line.contains("512 B / 1.00 KiB"));
        assert!(line.contains("50.00%"));
        assert!(line.contains("512 B/s"));
        let stat = AccountStat {
            status: "success",
            total: 6,
            transferred: 6,
            duration: 1_000_000_000,
            speed: 6.0,
        };
        let table = stat.table();
        assert!(table.contains("│ Total │ Transferred │ Duration │ Speed │"));
        assert!(table.contains("│ 6 B   │ 6 B         │ 00m01s   │ 6 B/s │"));
    }

    #[test]
    fn reader_counts_bytes() {
        use tokio::io::AsyncReadExt;
        let progress = Progress::new(4, false);
        let mut reader = ProgressReader::new(&b"data"[..], progress.clone());
        let mut out = Vec::new();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(reader.read_to_end(&mut out))
            .unwrap();
        assert_eq!(progress.transferred(), 4);
    }
}
