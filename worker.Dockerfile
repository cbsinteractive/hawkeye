FROM osig/rust-ubuntu:1.44.1 AS builder
RUN apt update
RUN apt install -y \
        libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
              gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
              gstreamer1.0-plugins-bad gstreamer1.0-libav libgstrtspserver-1.0-dev libges-1.0-dev
COPY Cargo.toml /Cargo.toml
COPY Cargo.lock /Cargo.lock
COPY hawkeye-api /hawkeye-api
COPY hawkeye-core /hawkeye-core
COPY hawkeye-worker /hawkeye-worker
COPY resources /resources
RUN cargo build --release --package hawkeye-worker

FROM ubuntu:18.04 AS app
RUN apt update \
    && apt install -y \
           gstreamer1.0-plugins-base \
           gstreamer1.0-plugins-good \
           gstreamer1.0-plugins-bad \
           gstreamer1.0-libav \
    && apt-get clean
ENV RUST_LOG=info
COPY --from=builder /target/release/hawkeye-worker .
ENTRYPOINT ["/hawkeye-worker"]
