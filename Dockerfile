# syntax=docker/dockerfile:1
# Stage 1: Build Rust binary
FROM rust:slim-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --locked

# Stage 2: Minimal runtime
FROM gcr.io/distroless/cc-debian12
COPY --from=builder /app/target/release/sparrow /usr/local/bin/sparrow
COPY --from=builder /etc/ssl/certs /etc/ssl/certs
EXPOSE 7443 7444 7445
USER nobody
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \
  CMD ["sparrow", "status"]
ENTRYPOINT ["sparrow"]
CMD ["--help"]
