# Linux check image for wi (native arm64/amd64, no cross-compiling).
FROM rust:1-slim-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \
      python3 python3-venv procps rsync bash ca-certificates locales openssh-client \
    && rm -rf /var/lib/apt/lists/*
RUN rustup component add clippy
RUN python3 -m venv /opt/venv && /opt/venv/bin/pip install --no-cache-dir pyte
ENV PATH="/opt/venv/bin:$PATH" CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target LANG=C.UTF-8
WORKDIR /work
