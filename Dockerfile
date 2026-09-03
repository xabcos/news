# syntax=docker/dockerfile:1

FROM rust:bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src/main.rs src/lib.rs ./src/
RUN cargo fetch --locked

COPY src ./src
RUN --mount=type=cache,id=news-target,target=/app/target,sharing=locked \
    cargo build --locked --release && \
    cp target/release/news /tmp/news

# Precompressed variants are served by ServeDir::precompressed_gzip at zero runtime cost.
COPY pages ./pages
RUN find pages -type f \( -name '*.js' -o -name '*.css' -o -name '*.html' -o -name '*.svg' \) \
    -exec gzip -kf9 {} +

FROM gcr.io/distroless/cc-debian13:nonroot
WORKDIR /home/nonroot

COPY --from=builder /tmp/news ./news
COPY migrations ./migrations
COPY --from=builder /app/pages ./pages

ENV BIND_ADDR=0.0.0.0:8788 \
    MIGRATIONS_DIR=/home/nonroot/migrations \
    PAGES_DIR=/home/nonroot/pages
EXPOSE 8788
ENTRYPOINT ["./news"]
