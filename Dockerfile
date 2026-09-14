FROM rust:1.97-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 mx
COPY --from=build /src/target/release/mx /usr/local/bin/mx
RUN ln -s /usr/local/bin/mx /usr/local/bin/mc
USER mx
ENTRYPOINT ["mc"]
