# Build stage  
FROM rust:latest as builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    build-essential \
    libc6-dev \
    && rm -rf /var/lib/apt/lists/*

# Set the working directory
WORKDIR /app

# Copy the Cargo.toml and Cargo.lock
COPY Cargo.toml ./
# Let Docker generate a compatible Cargo.lock
RUN cargo fetch

# Copy the source code
COPY src ./src

# Build the application in release mode
RUN cargo build --release

# Runtime stage - use newer base with compatible GLIBC
FROM ubuntu:24.04

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create a non-root user
RUN useradd -m -s /bin/bash ipc-benchmark

# Set up shared memory with proper permissions
RUN mkdir -p /dev/shm && \
    chmod 1777 /dev/shm

# Copy the binary from the builder stage
COPY --from=builder /app/target/release/ipc-benchmark /usr/local/bin/ipc-benchmark

# Make the binary executable
RUN chmod +x /usr/local/bin/ipc-benchmark

# Create directories for output and IPC sockets
RUN mkdir -p /app/output /app/sockets /tmp/ipc && \
    chown -R ipc-benchmark:ipc-benchmark /app && \
    chmod 755 /app/sockets /tmp/ipc

# Switch to non-root user
USER ipc-benchmark

# Set the working directory
WORKDIR /app

# Set environment variables
ENV RUST_LOG=info
ENV IPC_BENCHMARK_TEMP_DIR=/tmp/ipc
ENV IPC_BENCHMARK_OUTPUT_DIR=/app/output
ENV IPC_BENCHMARK_SOCKET_DIR=/app/sockets
ENV IPC_BENCHMARK_DEFAULT_SOCKET_PATH=/app/sockets/ipc_benchmark.sock

# Expose no ports by default (IPC is local)
# TCP tests will use ephemeral ports

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD ipc-benchmark --help || exit 1

# Default command
CMD ["ipc-benchmark", "--help"]

# Labels for better container management
LABEL maintainer="IPC Benchmark Contributors"
LABEL version="0.1.0"
LABEL description="IPC Benchmark Suite - A comprehensive tool for measuring IPC performance"
LABEL org.opencontainers.image.title="IPC Benchmark"
LABEL org.opencontainers.image.description="A comprehensive interprocess communication benchmark suite"
LABEL org.opencontainers.image.authors="IPC Benchmark Contributors"
LABEL org.opencontainers.image.source="https://github.com/your-org/ipc-benchmark"
LABEL org.opencontainers.image.documentation="https://github.com/your-org/ipc-benchmark/blob/main/README.md"
LABEL org.opencontainers.image.licenses="Apache-2.0" 