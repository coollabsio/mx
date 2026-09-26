use crate::commands::cat::{EncCFlag, ReadSelect, ignore_broken_pipe, open_s3, reject_s3_only};
use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::flags::{RewindFlag, VersionIdFlag};
use crate::location::{Location, parse_location};
use crate::s3::{BlockingReader, get_object};
use anyhow::{Context, Result, bail};
use clap::Args;
use std::io::{self, Write};
use std::time::SystemTime;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

#[derive(Debug, Args)]
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
    if json {
        bail!("`head` does not support `--json` yet.");
    }
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
        return ignore_broken_pipe(rt.block_on(head_stdin(lines, &mut out)));
    }
    let store = ConfigStore::load_or_create()?;
    for input in &args.targets {
        let result = rt.block_on(async {
            if input == "-" {
                return head_stdin(lines, &mut out).await;
            }
            match parse_location(input, store.config()) {
                Location::S3(target) => {
                    let alias = alias_config(&store, &target.alias)?;
                    let object = open_s3(&alias, &target, &select).await?;
                    let response =
                        get_object(&object.client, &object.bucket, &object.key, &object.get)
                            .await?;
                    write_lines(response.body.into_async_read(), lines, &mut out).await
                }
                Location::Local(path) => {
                    reject_s3_only(select.is_set(), "head")?;
                    let file = tokio::fs::File::open(&path)
                        .await
                        .with_context(|| format!("Unable to open `{}`.", path.display()))?;
                    write_lines(file, lines, &mut out).await
                }
            }
        });
        ignore_broken_pipe(result.with_context(|| format!("Unable to read from `{input}`.")))?;
    }
    Ok(())
}

async fn head_stdin(lines: u64, out: &mut impl Write) -> Result<()> {
    write_lines(BlockingReader::new(io::stdin().lock()), lines, out).await
}

/// Writes the first `lines` lines of `reader`, normalizing `\r\n` to `\n` and terminating the
/// last line with `\n` (like mc's `bufio.ReadLine` loop).
async fn write_lines<R: AsyncRead + Unpin>(
    reader: R,
    lines: u64,
    out: &mut impl Write,
) -> Result<()> {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    for _ in 0..lines {
        line.clear();
        if reader.read_until(b'\n', &mut line).await? == 0 {
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
    use super::write_lines;

    fn head(input: &[u8], lines: u64) -> Vec<u8> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut out = Vec::new();
        rt.block_on(write_lines(input, lines, &mut out)).unwrap();
        out
    }

    #[test]
    fn prints_first_lines() {
        assert_eq!(head(b"a\nb\r\nc\n", 2), b"a\nb\n");
        assert_eq!(head(b"a\nb", 5), b"a\nb\n");
        assert_eq!(head(b"a\nb\n", 0), b"");
        assert_eq!(head(b"", 3), b"");
    }
}
