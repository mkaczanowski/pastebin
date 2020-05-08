FROM rustlang/rust:nightly as builder

RUN apt-get update && apt-get install -y apt-utils software-properties-common lsb-release
RUN bash -c "$(wget -O - https://apt.llvm.org/llvm.sh)"

WORKDIR /usr/src/pastebin
COPY . .

RUN cargo install --path .

FROM debian:buster-slim
COPY --from=builder /usr/local/cargo/bin/pastebin /usr/local/bin/pastebin

ENTRYPOINT ["pastebin"]
CMD ["--help"]
