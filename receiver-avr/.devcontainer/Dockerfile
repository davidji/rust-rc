FROM debian:buster
RUN apt-get update
RUN apt-get install -y build-essential git python wget curl cmake libffi-dev
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --default-toolchain nightly -y
ENV PATH="/root/.cargo/bin:${PATH}"

