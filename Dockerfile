# syntax=docker/dockerfile:1
FROM rust:1.97-alpine AS build
RUN apk add --no-cache build-base cmake perl
WORKDIR /src
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
# Version data for `mx -v` (.git is not in the build context); see build.rs.
# Release builds pass the tagged commit and its commit time.
ARG MX_COMMIT_ID=
ARG SOURCE_DATE_EPOCH=
ARG MX_RELEASE=
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    # An empty SOURCE_DATE_EPOCH breaks C builds that use __DATE__ (GCC), so drop unset ARGs.
    for var in MX_COMMIT_ID SOURCE_DATE_EPOCH MX_RELEASE; do \
        eval "[ -n \"\$$var\" ]" || unset "$var"; \
    done \
    && cargo build --locked --release \
    && cp /src/target/release/mx /mx \
    && file /mx | grep -E 'static(-pie)? linked|statically linked'

FROM alpine:3.21
RUN apk add --no-cache ca-certificates \
    && adduser -D -u 10001 mx \
    && mkdir /home/mx/.mx \
    && chown mx:mx /home/mx/.mx
COPY --from=build /mx /usr/bin/mc
RUN ln -s /usr/bin/mc /usr/bin/mx
USER mx
ENTRYPOINT ["mc"]
