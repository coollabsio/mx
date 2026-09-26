# Lessons

- Container smoke tests for an S3 client must run real bucket and object operations against an S3-compatible server. CLI-only checks are not sufficient.
- A compatibility binary copied into other container images must be statically linked, exist at the exact source path consumers copy, and be tested on every published architecture.
- MinIO can reject unsigned AWS SDK metadata headers (`amz-sdk-invocation-id`, `amz-sdk-request`). The SDK adds them in its own `modify_before_transmit` hooks (after signing), so strip them in a client interceptor's `modify_before_transmit` (client interceptors run after the SDK's); stripping in `modify_before_signing` is a no-op. Verify with `mx --debug`.
- Official MinIO images (quay.io/minio, dl.min.io) are no longer pullable. Tests pin a community build in `tests/minio.image` (override with `MX_MINIO_IMAGE`); some features (for example bucket CORS) may answer `NotImplemented` there.
- Object keys can end in `/` (folder markers). Never rebuild absolute keys by trimming or joining without keeping the trailing slash; a lost slash makes destructive commands hit a sibling key.
