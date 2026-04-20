# Build stage
FROM rust:alpine AS builder

WORKDIR /app

RUN apk add --no-cache \
    build-base \
    clang \
    mold \
    pkgconfig

# Linux
RUN rustup target add x86_64-unknown-linux-gnu

COPY --from=ghcr.io/casey/just:latest /just /usr/local/bin/

COPY . .

RUN just build-release

# Runtime stage
FROM alpine:latest

WORKDIR /app

COPY --from=builder /app/bin /app/bin/

CMD ["/app/bin/glowing-spork"]
