FROM rustlang/rust:nightly-bookworm-slim as builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    clang llvm libclang-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/pastebin
COPY . .

RUN cargo install --path .

FROM debian:bookworm-slim
COPY --from=builder /usr/local/cargo/bin/pastebin /usr/local/bin/pastebin

ENTRYPOINT ["pastebin"]
CMD ["--help"]
