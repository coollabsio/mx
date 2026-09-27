//! Streaming upload engine (areas B/E): PutObject for small inputs, multipart for large ones,
//! with parallel part uploads (minio-go defaults). Local files are uploaded from file ranges
//! ([`upload_file`]); other streams are buffered one part at a time ([`upload_stream`]).

use super::objects::{
    PutOptions, PutOutcome, apply_put_options, checksum_override, effective_checksum,
    put_object_with, sse_c_headers,
};
use crate::flags::{ChecksumAlgo, Sse};
use crate::progress::{Progress, ProgressReader};
use crate::s3::S3ResultExt;
use anyhow::{Context, Result, bail};
use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::{ByteStream, Length};
use aws_sdk_s3::types::{ChecksumType, CompletedMultipartUpload, CompletedPart};
use std::io::Read;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

/// S3 limits.
pub const MIN_PART_SIZE: u64 = 5 * 1024 * 1024;
pub const MAX_PART_SIZE: u64 = 5 * 1024 * 1024 * 1024;
pub const MAX_PARTS: i32 = 10_000;
pub const MAX_SINGLE_PUT_SIZE: u64 = 5 * 1024 * 1024 * 1024;
pub const MAX_OBJECT_SIZE: u64 = 5 * 1024 * 1024 * 1024 * 1024;
/// minio-go `minPartSize`: default part size and single-PUT threshold.
pub const DEFAULT_PART_SIZE: u64 = 16 * 1024 * 1024;
/// minio-go `totalWorkers`: parts uploaded concurrently unless [`PutOptions::parallel`] is set.
pub const DEFAULT_PARALLEL: usize = 4;
/// Read size for file-range part bodies.
const FILE_READ_BUFFER: usize = 1024 * 1024;

/// minio-go `OptimalPartInfo`: `fixed` wins; otherwise `ceil(size / 10000)` rounded up to a
/// multiple of 16 MiB (at least 16 MiB). Unknown sizes assume the 5 TiB maximum (528 MiB).
pub fn optimal_part_size(size: Option<u64>, fixed: Option<u64>) -> u64 {
    if let Some(size) = fixed {
        return size;
    }
    let total = size.unwrap_or(MAX_OBJECT_SIZE);
    ((total / MAX_PARTS as u64).div_ceil(DEFAULT_PART_SIZE) * DEFAULT_PART_SIZE)
        .max(DEFAULT_PART_SIZE)
}

/// minio-go `putObjectCommon`: inputs of at most one (configured or default) part size are
/// sent with a single PutObject.
fn single_put_threshold(options: &PutOptions) -> u64 {
    options.part_size.unwrap_or(DEFAULT_PART_SIZE)
}

/// Largest single read of a [`BlockingReader`] (a full pipe buffer).
const BLOCKING_READ_SIZE: usize = 64 * 1024;

/// Adapts a blocking `std::io::Read` to `AsyncRead` (reads inline; fine under `block_on`).
pub struct BlockingReader<R>(R);

impl<R> BlockingReader<R> {
    pub fn new(reader: R) -> Self {
        Self(reader)
    }
}

impl<R: Read + Unpin> AsyncRead for BlockingReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        // Initialize (zero) at most one read's worth: parts are read into large uninitialized
        // buffers, and zeroing all of them per read would be quadratic.
        let len = buf.remaining().min(BLOCKING_READ_SIZE);
        loop {
            match this.0.read(buf.initialize_unfilled_to(len)) {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Poll::Ready(Err(error)),
                Ok(count) => {
                    buf.advance(count);
                    return Poll::Ready(Ok(()));
                }
            }
        }
    }
}

/// Reads up to `limit` bytes (fewer only at EOF). The buffer is sized once for `expected`
/// bytes (or `limit`, capped at 1 GiB, when unknown); untouched capacity costs no memory.
pub async fn read_part<R: AsyncRead + Unpin>(
    reader: &mut R,
    limit: u64,
    expected: Option<u64>,
) -> Result<Vec<u8>> {
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let initial = expected
        .map_or(1 << 30, |size| size.saturating_add(1))
        .min(limit as u64) as usize;
    let mut data = Vec::with_capacity(initial);
    while data.len() < limit {
        if data.len() == data.capacity() {
            data.reserve_exact(data.capacity().max(1 << 20).min(limit - data.len()));
        }
        let room = (data.capacity() - data.len()).min(limit - data.len()) as u64;
        if (&mut *reader).take(room).read_buf(&mut data).await? == 0 {
            break;
        }
    }
    Ok(data)
}

fn validate_part_size(options: &PutOptions) -> Result<()> {
    if let Some(size) = options.part_size
        && !(MIN_PART_SIZE..=MAX_PART_SIZE).contains(&size)
    {
        bail!("part size must be between 5 MiB and 5 GiB");
    }
    Ok(())
}

/// Uploads a stream to `bucket/key` honoring every [`PutOptions`] field.
///
/// * `disable_multipart`: single PutObject (errors above 5 GiB).
/// * Known sizes up to the single-PUT threshold (16 MiB or `part_size`) and inputs that end
///   within the first part use PutObject; everything else a multipart upload with
///   `parallel` (default 4) concurrent parts, each buffered in memory. Failed uploads are
///   aborted.
pub async fn upload_stream<R: AsyncRead + Unpin>(
    client: &Client,
    bucket: &str,
    key: &str,
    mut reader: R,
    size_hint: Option<u64>,
    options: &PutOptions,
) -> Result<PutOutcome> {
    validate_part_size(options)?;
    if options.disable_multipart {
        let data = read_part(&mut reader, MAX_SINGLE_PUT_SIZE + 1, size_hint).await?;
        if data.len() as u64 > MAX_SINGLE_PUT_SIZE {
            bail!("input exceeds the 5 GiB single PUT limit; enable multipart");
        }
        return put_object_with(client, bucket, key, data, options).await;
    }
    let threshold = single_put_threshold(options);
    if let Some(size) = size_hint.filter(|&size| size <= threshold) {
        let data = read_part(&mut reader, threshold, Some(size)).await?;
        return put_object_with(client, bucket, key, data, options).await;
    }

    let part_size = optimal_part_size(size_hint, options.part_size);
    let first = read_part(&mut reader, part_size, None).await?;
    if (first.len() as u64) < part_size {
        return put_object_with(client, bucket, key, first, options).await;
    }
    let parts = PartSource::Stream {
        reader,
        first: Some(first),
        part_size,
    };
    multipart_upload(client, bucket, key, options, parts).await
}

/// Uploads the local file `path` of `size` bytes like minio-go's `ReadAt` path: multipart
/// parts are streamed from file ranges (read in 1 MiB chunks) instead of being buffered, with
/// `parallel` (default 4) concurrent parts. Small files, `disable_multipart` and uploads that
/// need a checksum (computed over the buffered part) go through [`upload_stream`].
/// `progress` counts the bytes as they are sent.
pub async fn upload_file(
    client: &Client,
    bucket: &str,
    key: &str,
    path: &Path,
    size: u64,
    options: &PutOptions,
    progress: Option<Arc<Progress>>,
) -> Result<PutOutcome> {
    validate_part_size(options)?;
    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("Unable to read local file `{}`.", path.display()))?;
    if options.disable_multipart
        || size <= single_put_threshold(options)
        || effective_checksum(options).is_some()
    {
        return match progress {
            Some(progress) => {
                let reader = ProgressReader::new(file, progress);
                upload_stream(client, bucket, key, reader, Some(size), options).await
            }
            None => upload_stream(client, bucket, key, file, Some(size), options).await,
        };
    }
    drop(file);

    let parts: PartSource<'_, tokio::io::Empty> = PartSource::File {
        path,
        size,
        part_size: optimal_part_size(Some(size), options.part_size),
        progress,
    };
    multipart_upload(client, bucket, key, options, parts).await
}

/// Where multipart parts come from.
enum PartSource<'a, R> {
    /// A stream read one buffered part at a time; `first` was read already.
    Stream {
        reader: R,
        first: Option<Vec<u8>>,
        part_size: u64,
    },
    /// Ranges of a local file, streamed from disk.
    File {
        path: &'a Path,
        size: u64,
        part_size: u64,
        progress: Option<Arc<Progress>>,
    },
}

impl<R: AsyncRead + Unpin> PartSource<'_, R> {
    /// Body and length of part `part_number` (1-based), or `None` after the last part.
    async fn next(&mut self, part_number: i32) -> Result<Option<(ByteStream, u64)>> {
        match self {
            PartSource::Stream {
                reader,
                first,
                part_size,
            } => {
                let data = match first.take() {
                    Some(data) => data,
                    None => read_part(reader, *part_size, None).await?,
                };
                let len = data.len() as u64;
                Ok((len > 0).then(|| (ByteStream::from(data), len)))
            }
            PartSource::File {
                path,
                size,
                part_size,
                progress,
            } => {
                let offset = (part_number as u64 - 1) * *part_size;
                if offset >= *size {
                    return Ok(None);
                }
                let len = (*part_size).min(*size - offset);
                let body = ByteStream::read_from()
                    .path(*path)
                    .offset(offset)
                    .length(Length::Exact(len))
                    .buffer_size(FILE_READ_BUFFER)
                    .build()
                    .await
                    .with_context(|| format!("Unable to read local file `{}`.", path.display()))?;
                let body = match progress {
                    Some(progress) => ByteStream::new(crate::progress::count_body(
                        body.into_inner(),
                        progress.clone(),
                    )),
                    None => body,
                };
                Ok(Some((body, len)))
            }
        }
    }
}

struct PartContext {
    client: Client,
    bucket: String,
    key: String,
    upload_id: String,
    sse_c: Option<[u8; 32]>,
    checksum: Option<ChecksumAlgo>,
}

/// Multipart upload of the parts of `parts_source`, keeping at most `parallel` parts in flight.
async fn multipart_upload<R: AsyncRead + Unpin>(
    client: &Client,
    bucket: &str,
    key: &str,
    options: &PutOptions,
    mut parts_source: PartSource<'_, R>,
) -> Result<PutOutcome> {
    let headers = options.headers()?;
    let checksum = effective_checksum(options);
    let mut request = apply_put_options!(
        client.create_multipart_upload().bucket(bucket).key(key),
        options,
        &headers
    );
    if let Some(algorithm) = checksum {
        request = request.checksum_algorithm(algorithm.to_sdk());
        if algorithm == ChecksumAlgo::Crc64Nvme {
            request = request.checksum_type(ChecksumType::FullObject);
        }
    }
    let created = request.send().await.s3_object(bucket, "")?;
    let upload_id = created
        .upload_id()
        .context("S3 did not return a multipart upload ID")?
        .to_string();
    let context = Arc::new(PartContext {
        client: client.clone(),
        bucket: bucket.to_string(),
        key: key.to_string(),
        upload_id: upload_id.clone(),
        sse_c: options.sse.as_ref().and_then(Sse::customer_key),
        checksum,
    });

    let parallel = options.parallel.unwrap_or(DEFAULT_PARALLEL).max(1);
    let result = async {
        let mut tasks = tokio::task::JoinSet::new();
        let mut parts = Vec::new();
        let mut part_number: i32 = 1;
        let mut total: u64 = 0;
        loop {
            while tasks.len() >= parallel {
                if let Some(joined) = tasks.join_next().await {
                    parts.push(joined??);
                }
            }
            let Some((body, len)) = parts_source.next(part_number).await? else {
                break;
            };
            if part_number > MAX_PARTS {
                bail!("input exceeds the S3 multipart part limit");
            }
            total += len;
            if total > MAX_OBJECT_SIZE {
                bail!("input exceeds the S3 object size limit");
            }
            let context = context.clone();
            let number = part_number;
            tasks.spawn(async move { upload_part(&context, number, body).await });
            part_number += 1;
        }
        while let Some(joined) = tasks.join_next().await {
            parts.push(joined??);
        }
        parts.sort_by_key(|part: &CompletedPart| part.part_number());

        let mut complete = client
            .complete_multipart_upload()
            .bucket(bucket)
            .key(key)
            .upload_id(&upload_id)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(parts))
                    .build(),
            );
        if let Some(sse_key) = &context.sse_c {
            let (algorithm, encoded, md5) = sse_c_headers(sse_key);
            complete = complete
                .sse_customer_algorithm(algorithm)
                .sse_customer_key(encoded)
                .sse_customer_key_md5(md5);
        }
        let response = complete.send().await.s3_object(&context.bucket, "")?;
        Ok::<PutOutcome, anyhow::Error>(PutOutcome {
            size: Some(total as i64),
            etag: response.e_tag().map(str::to_string),
            version_id: response.version_id().map(str::to_string),
        })
    }
    .await;

    if result.is_err() {
        let _ = client
            .abort_multipart_upload()
            .bucket(bucket)
            .key(key)
            .upload_id(&upload_id)
            .send()
            .await;
    }
    result
}

async fn upload_part(
    context: &PartContext,
    part_number: i32,
    body: ByteStream,
) -> Result<CompletedPart> {
    let mut request = context
        .client
        .upload_part()
        .bucket(&context.bucket)
        .key(&context.key)
        .upload_id(&context.upload_id)
        .part_number(part_number)
        .body(body);
    if let Some(key) = &context.sse_c {
        let (algorithm, encoded, md5) = sse_c_headers(key);
        request = request
            .sse_customer_algorithm(algorithm)
            .sse_customer_key(encoded)
            .sse_customer_key_md5(md5);
    }
    let response = if let Some(algorithm) = context.checksum {
        request
            .checksum_algorithm(algorithm.to_sdk())
            .customize()
            .config_override(checksum_override())
            .send()
            .await
            .s3_object(&context.bucket, "")?
    } else {
        request.send().await.s3_object(&context.bucket, "")?
    };
    Ok(CompletedPart::builder()
        .part_number(part_number)
        .set_e_tag(response.e_tag().map(str::to_string))
        .set_checksum_crc32(response.checksum_crc32().map(str::to_string))
        .set_checksum_crc32_c(response.checksum_crc32_c().map(str::to_string))
        .set_checksum_crc64_nvme(response.checksum_crc64_nvme().map(str::to_string))
        .set_checksum_sha1(response.checksum_sha1().map(str::to_string))
        .set_checksum_sha256(response.checksum_sha256().map(str::to_string))
        .build())
}

#[cfg(test)]
mod tests {
    use super::{BlockingReader, DEFAULT_PART_SIZE, optimal_part_size, read_part};
    use std::io::{self, Read};

    #[test]
    fn retries_interrupted_reader() {
        struct InterruptedOnce(bool, bool);
        impl Read for InterruptedOnce {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if !self.0 {
                    self.0 = true;
                    return Err(io::ErrorKind::Interrupted.into());
                }
                if self.1 {
                    return Ok(0);
                }
                self.1 = true;
                buffer[..4].copy_from_slice(b"data");
                Ok(4)
            }
        }

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut reader = BlockingReader::new(InterruptedOnce(false, false));
        let output = runtime.block_on(read_part(&mut reader, 16, None)).unwrap();
        assert_eq!(output, b"data");
    }

    #[test]
    fn reads_parts_up_to_the_limit() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut input = &[5u8; 100][..];
        let part = runtime
            .block_on(read_part(&mut input, 64, Some(10)))
            .unwrap();
        assert_eq!(part.len(), 64);
        let part = runtime.block_on(read_part(&mut input, 64, None)).unwrap();
        assert_eq!(part.len(), 36);
    }

    #[test]
    fn part_size_follows_minio_go() {
        const MIB: u64 = 1024 * 1024;
        assert_eq!(optimal_part_size(Some(10), None), DEFAULT_PART_SIZE);
        assert_eq!(optimal_part_size(Some(2048 * MIB), None), 16 * MIB);
        // Unknown size assumes 5 TiB: mc pipe's "528 MiB".
        assert_eq!(optimal_part_size(None, None), 528 * MIB);
        let one_tib = 1024_u64.pow(4);
        let size = optimal_part_size(Some(one_tib), None);
        assert!(size * 10_000 >= one_tib);
        assert_eq!(size % (16 * MIB), 0);
        assert_eq!(optimal_part_size(Some(one_tib), Some(6 * MIB)), 6 * MIB);
    }
}
