FROM rust:1.97-alpine AS build
RUN apk add --no-cache build-base cmake perl
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release \
    && file target/release/mx | grep -E 'static(-pie)? linked|statically linked'

FROM alpine:3.21
RUN apk add --no-cache ca-certificates \
    && adduser -D -u 10001 mx \
    && mkdir /home/mx/.mx \
    && chown mx:mx /home/mx/.mx
COPY --from=build /src/target/release/mx /usr/bin/mc
RUN ln -s /usr/bin/mc /usr/bin/mx
USER mx
ENTRYPOINT ["mc"]
