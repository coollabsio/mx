use crate::commands::{alias_config, runtime};
use crate::config::ConfigStore;
use crate::flags::{EncFlags, RewindFlag, Sse, VersionIdFlag, resolve_sse};
use crate::location::{Location, parse_location};
use crate::s3::{GetOptions, get_object, head_object_with};
use crate::target::TargetRef;
use anyhow::{Context, Result, bail};
use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::ByteStream;
use clap::Args;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::time::SystemTime;

#[derive(Debug, Args)]
#[command(mut_args(|a| match a.get_id().as_str() {
    "rewind" => a.help("display an earlier object version"),
    "version_id" => a.help("display a specific version of an object"),
    _ => a,
}))]
pub struct CatArgs {
    #[command(flatten)]
    pub rewind: RewindFlag,
    #[command(flatten)]
    pub version: VersionIdFlag,
    /// extract from remote zip file (MinIO server source only)
    #[arg(long)]
    pub zip: bool,
    /// start offset
    #[arg(
        long,
        value_name = "N",
        default_value_t = 0,
        allow_negative_numbers = true
    )]
    pub offset: i64,
    /// tail number of bytes at ending of file
    #[arg(
        long,
        value_name = "N",
        default_value_t = 0,
        allow_negative_numbers = true
    )]
    pub tail: i64,
    /// download only a specific part number
    #[arg(long = "part-number", value_name = "N", default_value_t = 0)]
    pub part_number: i32,
    #[command(flatten)]
    pub enc: EncCFlag,
    /// objects or files to print; `-` or no target reads standard input
    pub targets: Vec<String>,
}

/// `--enc-c` only (read-side commands).
#[derive(Debug, Clone, Default, Args)]
pub struct EncCFlag {
    #[arg(
        long = "enc-c",
        value_name = "PATH=KEY",
        help = "encrypt/decrypt objects using client provided keys. (multiple keys can be provided) Formats: RawBase64 or Hex."
    )]
    pub enc_c: Vec<String>,
}

impl EncCFlag {
    pub fn entries(&self) -> Result<Vec<(String, Sse)>> {
        EncFlags {
            enc_c: self.enc_c.clone(),
            ..Default::default()
        }
        .entries()
    }
}

/// How to select and decrypt an S3 object for reading.
#[derive(Debug, Clone, Default)]
pub struct ReadSelect {
    pub version_id: Option<String>,
    pub rewind: Option<SystemTime>,
    pub zip: bool,
    pub enc: Vec<(String, Sse)>,
}

impl ReadSelect {
    pub fn is_set(&self) -> bool {
        self.version_id.is_some() || self.rewind.is_some() || self.zip
    }
}

/// A resolved S3 object ready for GetObject.
pub struct S3Object {
    pub client: Client,
    pub bucket: String,
    pub key: String,
    pub get: GetOptions,
    pub size: i64,
}

/// Resolves version (`--version-id` / `--rewind`) and SSE-C key, then stats the object.
pub async fn open_s3(
    alias: &crate::config::model::AliasConfig,
    target: &TargetRef,
    select: &ReadSelect,
) -> Result<S3Object> {
    let bucket = target.require_bucket()?.to_string();
    let key = target.require_object_key()?;
    let client = crate::s3::build_client(alias).await?;
    let mut get = GetOptions {
        version_id: select.version_id.clone(),
        sse_c: resolve_sse(&select.enc, &format!("{}/{bucket}/{key}", target.alias))
            .and_then(|sse| sse.customer_key()),
        zip_extract: select.zip,
        ..Default::default()
    };
    if let Some(at) = select.rewind {
        get.version_id = Some(crate::s3::version_at(&client, &bucket, &key, at).await?);
    }
    let head = head_object_with(&client, &bucket, &key, &get).await?;
    Ok(S3Object {
        size: head.content_length().unwrap_or(0),
        client,
        bucket,
        key,
        get,
    })
}

/// First byte to print for `--offset` / `--tail` on an object of `size` bytes.
pub fn start_offset(offset: i64, tail: i64, size: i64) -> Result<i64> {
    let start = if tail > 0 && size > 0 {
        (size - tail).max(0)
    } else {
        offset
    };
    if start > size {
        bail!("specified offset ({start}) bigger than file ({size})");
    }
    Ok(start)
}

fn validate(args: &CatArgs) -> Result<()> {
    if args.version.version_id.is_some() && args.rewind.rewind.is_some() {
        bail!("You cannot specify --version-id and --rewind at the same time");
    }
    if args.version.version_id.is_some() && args.targets.len() != 1 {
        bail!("You need to pass exactly one argument if --version-id is specified");
    }
    if args.tail != 0 && args.offset != 0 {
        bail!("You cannot specify both --tail and --offset");
    }
    if args.tail < 0 || args.offset < 0 {
        bail!("You cannot specify negative --tail or --offset");
    }
    if args.part_number < 0 {
        bail!("You cannot specify a negative --part-number");
    }
    if args.zip && (args.tail != 0 || args.offset != 0) {
        bail!("You cannot combine --zip with --tail or --offset");
    }
    let stdin = args.targets.is_empty() || args.targets.iter().any(|t| t == "-");
    if stdin && (args.zip || args.offset != 0 || args.tail != 0 || args.part_number != 0) {
        bail!("You cannot use --zip, --tail, --offset or --part-number with stdin");
    }
    if (args.tail != 0 || args.offset != 0) && args.part_number > 0 {
        bail!("You cannot use --part-number with --tail or --offset");
    }
    Ok(())
}

pub fn run(args: CatArgs, json: bool) -> Result<()> {
    // mc writes the raw contents with `--json` too; only errors are JSON.
    let _ = json;
    validate(&args)?;
    let select = ReadSelect {
        version_id: args.version.version_id.clone(),
        rewind: args.rewind.at(SystemTime::now())?,
        zip: args.zip,
        enc: args.enc.entries()?,
    };
    let mut out = io::stdout().lock();
    if args.targets.is_empty() {
        return ignore_broken_pipe(copy_stdin(&mut out));
    }
    let store = ConfigStore::load_or_create()?;
    for target in &args.targets {
        let result = cat_one(&store, target, &args, &select, &mut out);
        ignore_broken_pipe(result.with_context(|| format!("Unable to read from `{target}`.")))?;
    }
    Ok(())
}

fn cat_one(
    store: &ConfigStore,
    input: &str,
    args: &CatArgs,
    select: &ReadSelect,
    out: &mut impl Write,
) -> Result<()> {
    if input == "-" {
        return copy_stdin(out);
    }
    match parse_location(input, store.config()) {
        Location::S3(target) => {
            let alias = alias_config(store, &target.alias)?;
            runtime()?.block_on(async {
                let mut object = open_s3(&alias, &target, select).await?;
                let start = start_offset(args.offset, args.tail, object.size)?;
                let mut expected = Some(object.size - start);
                if args.part_number > 0 {
                    object.get.part_number = Some(args.part_number);
                    expected = None;
                }
                if start > 0 {
                    if start == object.size {
                        return Ok(());
                    }
                    object.get.range = Some((start as u64, None));
                }
                let response =
                    get_object(&object.client, &object.bucket, &object.key, &object.get).await?;
                let written = write_body(response.body, out).await?;
                if let Some(expected) = expected
                    && written != expected as u64
                {
                    bail!("unexpected EOF: wrote {written} of {expected} bytes");
                }
                Ok(())
            })
        }
        Location::Local(path) => {
            reject_s3_only(select.is_set() || args.part_number != 0, "cat")?;
            let mut file = std::fs::File::open(&path)
                .map_err(|error| crate::error::io_error(&error, &path.to_string_lossy()))?;
            let size = file.metadata()?.len() as i64;
            let start = start_offset(args.offset, args.tail, size)?;
            file.seek(SeekFrom::Start(start as u64))?;
            io::copy(&mut file, out)?;
            out.flush()?;
            Ok(())
        }
    }
}

/// Fails when S3-only read flags are used with a local path.
pub fn reject_s3_only(s3_flags_set: bool, command: &str) -> Result<()> {
    if s3_flags_set {
        bail!(
            "`{command}` flags --version-id, --rewind, --zip and --part-number are only supported for S3 objects"
        );
    }
    Ok(())
}

fn copy_stdin(out: &mut impl Write) -> Result<()> {
    let mut stdin = io::stdin().lock();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let count = stdin.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        out.write_all(&buffer[..count])?;
    }
    out.flush()?;
    Ok(())
}

/// Streams an object body to `out`; returns the number of bytes written.
pub async fn write_body(mut body: ByteStream, out: &mut impl Write) -> Result<u64> {
    let mut written = 0u64;
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        out.write_all(&chunk)?;
        written += chunk.len() as u64;
    }
    out.flush()?;
    Ok(written)
}

/// Treats a closed stdout (e.g. `mx cat ... | head`) as success, like mc.
pub fn ignore_broken_pipe(result: Result<()>) -> Result<()> {
    match result {
        Err(error)
            if error.chain().any(|cause| {
                cause
                    .downcast_ref::<io::Error>()
                    .is_some_and(|io| io.kind() == io::ErrorKind::BrokenPipe)
            }) =>
        {
            Ok(())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::start_offset;

    #[test]
    fn computes_offsets() {
        assert_eq!(start_offset(0, 0, 10).unwrap(), 0);
        assert_eq!(start_offset(4, 0, 10).unwrap(), 4);
        assert_eq!(start_offset(10, 0, 10).unwrap(), 10);
        assert!(start_offset(11, 0, 10).is_err());
        assert_eq!(start_offset(0, 3, 10).unwrap(), 7);
        assert_eq!(start_offset(0, 30, 10).unwrap(), 0);
        assert_eq!(start_offset(0, 3, 0).unwrap(), 0);
    }
}
