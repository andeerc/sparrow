# syntax=docker/dockerfile:1
# Stage 1: Build Rust binary
FROM rust:1.85-slim-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --locked

# Stage 2: Runtime image (minimal Debian)
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/sparrow /usr/local/bin/sparrow
EXPOSE 7443 7444 7445
ENTRYPOINT ["sparrow"]
CMD ["--help"]
