FROM rust:1.95-bookworm AS builder
WORKDIR /app

# Build the dependency graph first so this layer is reused when only source
# files change.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src \
    && printf 'fn main() {}\n' > src/main.rs \
    && cargo build --release \
    && rm -rf src target/release/deps/crier* target/release/crier*

COPY migrations ./migrations
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --create-home crier \
    && mkdir /data && chown crier /data
COPY --from=builder /app/target/release/crier /usr/local/bin/crier
USER 10001
VOLUME ["/data"]
ENV CRIER_DATABASE_URL=sqlite:///data/crier.sqlite
ENTRYPOINT ["/usr/local/bin/crier"]
CMD ["run"]
