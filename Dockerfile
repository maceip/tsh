# ==========================================================================
# Multi-stage build for tsh (Token Shell)
# Compiles Rust binaries, installs Python dependencies, and produces a
# minimal runtime image with both the Rust CLI and the Python compliance shim.
# ==========================================================================

# --- Stage 1: Build Rust binaries ---
FROM rust:1.77-bookworm AS rust-builder

WORKDIR /build

# Copy manifests first to cache dependency compilation
COPY Cargo.toml Cargo.lock* ./
COPY crates/langextract-host/Cargo.toml crates/langextract-host/Cargo.toml
COPY crates/tsh/Cargo.toml crates/tsh/Cargo.toml
COPY xtask/Cargo.toml xtask/Cargo.toml

# Create stub source files to compile dependencies
RUN mkdir -p crates/langextract-host/src crates/tsh/src xtask/src && \
    echo "pub fn chunk_text(_t: &str, _m: usize, _o: usize) -> Vec<&str> { vec![] }" > crates/langextract-host/src/lib.rs && \
    echo "fn main() {}" > crates/langextract-host/src/main.rs && \
    echo "fn main() {}" > crates/tsh/src/main.rs && \
    echo "fn main() {}" > xtask/src/main.rs && \
    cargo build --release -p tsh -p langextract-host 2>/dev/null || true

# Copy real source and rebuild
COPY crates/ crates/
COPY xtask/ xtask/
RUN touch crates/langextract-host/src/lib.rs crates/langextract-host/src/main.rs \
          crates/tsh/src/main.rs xtask/src/main.rs && \
    cargo build --release -p tsh -p langextract-host

# --- Stage 2: Runtime ---
FROM python:3.12-slim-bookworm

WORKDIR /app

# Install Python dependencies
COPY python/requirements.txt python/requirements.txt
RUN pip install --no-cache-dir -r python/requirements.txt

# Copy Rust binaries from builder
COPY --from=rust-builder /build/target/release/tsh /usr/local/bin/tsh
COPY --from=rust-builder /build/target/release/langextract-host /usr/local/bin/langextract-host

# Copy the Python compliance shim
COPY python/ python/

# Default environment for Ollama endpoint (override with docker-compose or -e)
ENV OPENAI_API_BASE="http://host.docker.internal:11434/v1"
ENV OPENAI_API_KEY="local-poc-key"

ENTRYPOINT ["tsh"]
