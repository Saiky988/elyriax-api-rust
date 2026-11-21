# Stage 1: Build optimized Rust binary
FROM rust:1.85-bookworm AS builder

WORKDIR /app
COPY Cargo.toml ./
COPY src ./src

RUN cargo build --release

# Stage 2: Minimal runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/*

ENV TZ=Asia/Ho_Chi_Minh
WORKDIR /app

COPY --from=builder /app/target/release/elyriax-api /app/elyriax-api
COPY data /app/data
COPY templates /app/templates
RUN mkdir -p /app/uploads

EXPOSE 25243
CMD ["/app/elyriax-api"]
