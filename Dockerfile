FROM rust:1-alpine AS builder

RUN USER=root apk add --no-cache musl-dev openssl-dev

# https://github.com/sfackler/rust-openssl/issues/1462#issuecomment-826089294
ENV RUSTFLAGS=-Ctarget-feature=-crt-static

# Make a fake Rust app to keep a cached layer of compiled crates
RUN USER=root cargo new app
WORKDIR /usr/src/app
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src/bin && echo "fn main(){}" > src/bin/nwws-http-server.rs
RUN cargo build --release --all-features

COPY . .
RUN cargo install --path . --all-features

# Runtime image
FROM alpine

RUN USER=root apk add --no-cache openssl ca-certificates libgcc libc6-compat && \
    addgroup -S app && adduser -S app -G app

USER app
WORKDIR /app

COPY --from=builder /usr/local/cargo/bin/nwws-http-server /usr/local/bin/nwws-http-server
CMD /usr/local/bin/nwws-http-server

