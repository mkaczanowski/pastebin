FROM rustlang/rust:nightly

RUN apt-get update && apt-get install -y apt-utils software-properties-common lsb-release
RUN bash -c "$(wget -O - https://apt.llvm.org/llvm.sh)"

WORKDIR /usr/src/pastebin
COPY . .

RUN cargo install --path .

CMD ["pastebin"]
