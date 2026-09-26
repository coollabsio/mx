use crate::commands::cat::{EncCFlag, ReadSelect, ignore_broken_pipe, open_s3, reject_s3_only};
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::flags::{RewindFlag, VersionIdFlag};
use crate::location::{Location, parse_location};
use crate::s3::get_object;
use anyhow::{Context, Result, bail};
use aws_sdk_s3::primitives::ByteStream;
use bytes::Bytes;
use clap::Args;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::time::SystemTime;

#[derive(Debug, Args)]
#[command(mut_args(|a| match a.get_id().as_str() {
    "rewind" => a.help("select an object version at specified time"),
    "version_id" => a.help("select an object version to display"),
    _ => a,
}))]
pub struct HeadArgs {
    /// print the first 'n' lines
    #[arg(short = 'n', long, default_value_t = 10, allow_negative_numbers = true)]
    pub lines: i64,
    #[command(flatten)]
    pub rewind: RewindFlag,
    #[command(flatten)]
    pub version: VersionIdFlag,
    /// extract from remote zip file (MinIO server source only)
    #[arg(long)]
    pub zip: bool,
    #[command(flatten)]
    pub enc: EncCFlag,
    /// objects or files to read; `-` or no target reads standard input
    pub targets: Vec<String>,
}

pub fn run(args: HeadArgs, json: bool) -> Result<()> {
    // mc writes the raw contents with `--json` too; only errors are JSON.
    let _ = json;
    if args.version.version_id.is_some() && args.rewind.rewind.is_some() {
        bail!("You cannot specify --version-id and --rewind at the same time");
    }
    if args.version.version_id.is_some() && args.targets.len() != 1 {
        bail!("You need to pass exactly one argument if --version-id is specified");
    }
    // mc: negative line counts fall back to the default.
    let lines = if args.lines < 0 {
        10
    } else {
        args.lines as u64
    };
    let select = ReadSelect {
        version_id: args.version.version_id.clone(),
        rewind: args.rewind.at(SystemTime::now())?,
        zip: args.zip,
        enc: args.enc.entries()?,
    };
    let rt = runtime()?;
    let mut out = io::stdout().lock();
    if args.targets.is_empty() {
        return ignore_broken_pipe(write_lines(io::stdin().lock(), lines, &mut out));
    }
    let store = ConfigStore::load_or_create()?;
    for input in &args.targets {
        let result = (|| {
            if input == "-" {
                return write_lines(io::stdin().lock(), lines, &mut out);
            }
            let (reader, content_type): (Box<dyn Read>, String) =
                match parse_location(input, store.config()) {
                    Location::S3(target) => {
                        let alias = alias_config(&store, &target.alias)?;
                        let response = rt.block_on(async {
                            let object = open_s3(&alias, &target, &select).await?;
                            get_object(&object.client, &object.bucket, &object.key, &object.get)
                                .await
                        })?;
                        let content_type = response.content_type().unwrap_or_default().to_string();
                        let body = BodyReader {
                            rt: &rt,
                            body: response.body,
                            chunk: Bytes::new(),
                        };
                        (Box::new(body), content_type)
                    }
                    Location::Local(path) => {
                        reject_s3_only(select.is_set(), "head")?;
                        let file = std::fs::File::open(&path).map_err(|error| {
                            crate::error::io_error(&error, &path.to_string_lossy())
                        })?;
                        (Box::new(file), crate::s3::guess_content_type(&path))
                    }
                };
            write_lines(
                BufReader::new(decompress(reader, &content_type)),
                lines,
                &mut out,
            )
        })();
        ignore_broken_pipe(result.with_context(|| format!("Unable to read from `{input}`.")))?;
    }
    Ok(())
}

/// mc `head` transparently decompresses objects whose Content-Type mentions `gzip` or `bzip`
/// (for local files the type is guessed from the extension).
fn decompress<'a>(reader: Box<dyn Read + 'a>, content_type: &str) -> Box<dyn Read + 'a> {
    if content_type.contains("gzip") {
        Box::new(flate2::read::MultiGzDecoder::new(reader))
    } else if content_type.contains("bzip") {
        Box::new(bzip2::read::MultiBzDecoder::new(reader))
    } else {
        reader
    }
}

/// Blocking reader over an S3 response body (driven by the command's runtime).
struct BodyReader<'a> {
    rt: &'a tokio::runtime::Runtime,
    body: ByteStream,
    chunk: Bytes,
}

impl Read for BodyReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.chunk.is_empty() {
            match self.rt.block_on(self.body.try_next()) {
                Ok(Some(chunk)) => self.chunk = chunk,
                Ok(None) => return Ok(0),
                Err(error) => return Err(io::Error::other(error)),
            }
        }
        let n = buf.len().min(self.chunk.len());
        buf[..n].copy_from_slice(&self.chunk.split_to(n));
        Ok(n)
    }
}

/// Writes the first `lines` lines of `reader`, normalizing `\r\n` to `\n` and terminating the
/// last line with `\n` (like mc's `bufio.ReadLine` loop).
fn write_lines(mut reader: impl BufRead, lines: u64, out: &mut impl Write) -> Result<()> {
    let mut line = Vec::new();
    for _ in 0..lines {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        if line.last() == Some(&b'\n') {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
        }
        out.write_all(&line)?;
        out.write_all(b"\n")?;
    }
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{decompress, write_lines};
    use std::io::{BufReader, Write};

    fn head(input: &[u8], lines: u64) -> Vec<u8> {
        let mut out = Vec::new();
        write_lines(input, lines, &mut out).unwrap();
        out
    }

    fn head_compressed(input: Vec<u8>, content_type: &str) -> Vec<u8> {
        let reader = BufReader::new(decompress(
            Box::new(std::io::Cursor::new(input)),
            content_type,
        ));
        let mut out = Vec::new();
        write_lines(reader, 2, &mut out).unwrap();
        out
    }

    #[test]
    fn decompresses_gzip_and_bzip2() {
        let text = b"one\ntwo\nthree\n";
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(text).unwrap();
        let gz = gz.finish().unwrap();
        assert_eq!(
            head_compressed(gz.clone(), "application/gzip"),
            b"one\ntwo\n"
        );
        assert_eq!(
            head_compressed(gz.clone(), "application/x-gzip"),
            b"one\ntwo\n"
        );
        let mut bz = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
        bz.write_all(text).unwrap();
        let bz = bz.finish().unwrap();
        assert_eq!(head_compressed(bz, "application/x-bzip2"), b"one\ntwo\n");
        // Other types are passed through untouched.
        assert_eq!(head_compressed(text.to_vec(), "text/plain"), b"one\ntwo\n");
        assert_ne!(
            head_compressed(gz, "application/octet-stream"),
            b"one\ntwo\n"
        );
    }

    #[test]
    fn prints_first_lines() {
        assert_eq!(head(b"a\nb\r\nc\n", 2), b"a\nb\n");
        assert_eq!(head(b"a\nb", 5), b"a\nb\n");
        assert_eq!(head(b"a\nb\n", 0), b"");
        assert_eq!(head(b"", 3), b"");
    }
}
