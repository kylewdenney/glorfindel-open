# Stage 1: Build the Rust binary
FROM rust:1.83-slim AS builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libzmq3-dev \
    cmake \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .

RUN cargo build --release

# Stage 2: Runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    libzmq5 \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/glorfindel /usr/local/bin/glorfindel

# Default environment
ENV OLLAMA_HOST=http://ollama:11434
ENV GLORFINDEL_MODELS=mistral
ENV DDS_DOMAIN_ID=0
ENV ZMQ_TOOL_CALL_ENDPOINT=tcp://0.0.0.0:5555
ENV ZMQ_TOOL_RESULT_ENDPOINT=tcp://0.0.0.0:5556
ENV RUST_LOG=info

EXPOSE 5555 5556

ENTRYPOINT ["glorfindel"]
