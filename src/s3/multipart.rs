//! Streaming upload engine (areas B/E): PutObject for small inputs, multipart for large ones,
//! with optional parallel part uploads. Wrap the reader to observe progress or throttle.

use super::objects::{
    PutOptions, PutOutcome, apply_put_options, checksum_override, effective_checksum,
    put_object_with, sse_c_headers,
};
use crate::flags::{ChecksumAlgo, Sse};
use crate::s3::S3ResultExt;
use anyhow::{Context, Result, bail};
use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{ChecksumType, CompletedMultipartUpload, CompletedPart};
use std::io::Read;
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
/// Default part size for unknown-size streams (grows every 1000 parts).
pub const DEFAULT_PART_SIZE: u64 = 8 * 1024 * 1024;

/// Part size for `part_number` (1-based). `fixed` wins; with a known size the part size is
/// `max(8 MiB, ceil(size / 10000))` rounded up to 1 MiB; otherwise 8 MiB doubling every 1000
/// parts (capped at 4 GiB) so unknown streams can reach the 5 TiB object limit.
pub fn part_size_for(part_number: i32, fixed: Option<u64>, size_hint: Option<u64>) -> u64 {
    if let Some(size) = fixed {
        return size;
    }
    if let Some(total) = size_hint {
        const MIB: u64 = 1024 * 1024;
        let needed = total.div_ceil(MAX_PARTS as u64).div_ceil(MIB) * MIB;
        return needed.max(DEFAULT_PART_SIZE);
    }
    let growth = ((part_number.saturating_sub(1) as u64) / 1_000).min(9);
    DEFAULT_PART_SIZE << growth
}

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
        loop {
            match this.0.read(buf.initialize_unfilled()) {
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

/// Reads up to `limit` bytes (fewer only at EOF).
pub async fn read_part<R: AsyncRead + Unpin>(reader: &mut R, limit: u64) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    reader.take(limit).read_to_end(&mut data).await?;
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
/// * Otherwise reads the first part; if the input ends within it, uses PutObject, else a
///   multipart upload with `parallel` concurrent parts. Failed uploads are aborted.
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
        let data = read_part(&mut reader, MAX_SINGLE_PUT_SIZE + 1).await?;
        if data.len() as u64 > MAX_SINGLE_PUT_SIZE {
            bail!("input exceeds the 5 GiB single PUT limit; enable multipart");
        }
        return put_object_with(client, bucket, key, data, options).await;
    }

    let first_size = part_size_for(1, options.part_size, size_hint);
    let first = read_part(&mut reader, first_size).await?;
    if (first.len() as u64) < first_size {
        return put_object_with(client, bucket, key, first, options).await;
    }
    multipart_upload(client, bucket, key, reader, first, size_hint, options).await
}

struct PartContext {
    client: Client,
    bucket: String,
    key: String,
    upload_id: String,
    sse_c: Option<[u8; 32]>,
    checksum: Option<ChecksumAlgo>,
}

async fn multipart_upload<R: AsyncRead + Unpin>(
    client: &Client,
    bucket: &str,
    key: &str,
    mut reader: R,
    first: Vec<u8>,
    size_hint: Option<u64>,
    options: &PutOptions,
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

    let parallel = options.parallel.unwrap_or(1).max(1);
    let result = async {
        let mut tasks = tokio::task::JoinSet::new();
        let mut parts = Vec::new();
        let mut part_number: i32 = 1;
        let mut total: u64 = 0;
        let mut data = first;
        loop {
            if part_number > MAX_PARTS {
                bail!("input exceeds the S3 multipart part limit");
            }
            total += data.len() as u64;
            if total > MAX_OBJECT_SIZE {
                bail!("input exceeds the S3 object size limit");
            }
            while tasks.len() >= parallel {
                if let Some(joined) = tasks.join_next().await {
                    parts.push(joined??);
                }
            }
            let context = context.clone();
            let number = part_number;
            tasks.spawn(async move { upload_part(&context, number, data).await });
            part_number += 1;
            data = read_part(
                &mut reader,
                part_size_for(part_number, options.part_size, size_hint),
            )
            .await?;
            if data.is_empty() {
                break;
            }
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
    data: Vec<u8>,
) -> Result<CompletedPart> {
    let mut request = context
        .client
        .upload_part()
        .bucket(&context.bucket)
        .key(&context.key)
        .upload_id(&context.upload_id)
        .part_number(part_number)
        .body(ByteStream::from(data));
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
    use super::{BlockingReader, DEFAULT_PART_SIZE, part_size_for, read_part};
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
        let output = runtime.block_on(read_part(&mut reader, 16)).unwrap();
        assert_eq!(output, b"data");
    }

    #[test]
    fn multipart_parts_grow_for_large_unknown_streams() {
        assert_eq!(part_size_for(1, None, None), 8 * 1024 * 1024);
        assert_eq!(part_size_for(1_000, None, None), 8 * 1024 * 1024);
        assert_eq!(part_size_for(1_001, None, None), 16 * 1024 * 1024);
        assert_eq!(part_size_for(9_001, None, None), 4 * 1024 * 1024 * 1024);
    }

    #[test]
    fn part_size_scales_with_known_size() {
        assert_eq!(part_size_for(1, None, Some(10)), DEFAULT_PART_SIZE);
        let one_tib = 1024_u64.pow(4);
        let size = part_size_for(1, None, Some(one_tib));
        assert!(size * 10_000 >= one_tib);
        assert_eq!(size % (1024 * 1024), 0);
        assert_eq!(part_size_for(3, Some(6 << 20), Some(one_tib)), 6 << 20);
    }
}
