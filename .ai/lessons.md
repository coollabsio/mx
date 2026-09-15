# Lessons

- Container smoke tests for an S3 client must run real bucket and object operations against an S3-compatible server. CLI-only checks are not sufficient.
- A compatibility binary copied into other container images must be statically linked, exist at the exact source path consumers copy, and be tested on every published architecture.
