[![Build Status][travis-badge]][travis]
[![Docker Cloud Build Status][docker-cloud-build-status]][docker-hub]
[![Docker Pulls][docker-pulls]][docker-hub]
[![Docker Image Size][docker-size]][docker-hub]

[travis-badge]: https://travis-ci.org/mkaczanowski/pastebin.svg?branch=master
[travis]: https://travis-ci.org/mkaczanowski/pastebin/
[docker-hub]: https://hub.docker.com/r/mkaczanowski/pastebin
[docker-cloud-build-status]: https://img.shields.io/docker/cloud/build/mkaczanowski/pastebin
[docker-pulls]: https://img.shields.io/docker/pulls/mkaczanowski/pastebin
[docker-size]: https://img.shields.io/docker/image-size/mkaczanowski/pastebin

# Pastebin
Simple, fast, standalone pastebin service.

## Why?
Whenever you need to share a code snippet, diff, logs, or a secret with another human being, the Pastebin service is invaluable. However, using public services such as pastebin.com, privnote.com, etc. should be avoided when you're sharing data that should be available only for a selected audience (i.e., your company, private network). Instead of trusting external providers, you could host your own Pastebin service and take ownership of all your data!

**There are numerous [Pastebin implementations](https://github.com/awesome-selfhosted/awesome-selfhosted#pastebins) out there, why would you implement another one?**

While the other implementation is great, I couldn't find one that would satisfy my requirements:
* no dependencies - one binary is all I want, no python libs, ruby runtime magic, no javascript or external databases to setup
* storage - fast, lightweight, self-hosted key-value storage able to hold a lot of data.
* speed - it must be fast. Once deployed in a mid-sized company you can expect high(er) traffic with low latency expectations from users
* reliability - no one wants to fix things that should just work (and are that simple!)
* cheap - low-cost service that would not steal too much of CPU time, thus add up to your bill
* CLI + GUI - it must be easy to interface from both ends (but still, no deps!)
* other features:
** on-demand encryption
** syntax highlighting
** destroy after reading
** destroy after expiration date

This Pastebin implementation satisfies all of the above requirements!

## Implementation
This is a rust version of Pastebin service with [rocksdb](https://rocksdb.org/) database as storage. In addition to previously mentioned features it's worth to mention:
* all-in-one binary - all the data, including css/javascript files are compiled into the binary. This way you don't need to worry about external dependencies, it's all witin. (see: [std::include_bytes](https://doc.rust-lang.org/std/macro.include_bytes.html))
* [REST endpoint](https://rocket.rs/) - you can add/delete pastes via standard HTTP client (ie. curl)
* [RocksDB compaction filter](https://github.com/facebook/rocksdb/wiki/Compaction-Filter) - the expired pastes will be automatically removed by custom compaction filter
* [flatbuffers](https://google.github.io/flatbuffers/) - data is serialized with flatbuffers (access to serialized data without parsing/unpacking)
* GUI - the UI is a plain HTML with [Bootstrap JS](https://getbootstrap.com/), [jQuery](https://jquery.com/) and [prism.js](https://prismjs.com/)
* Encryption - password-protected pastes are AES encrypted/decprypted in the browser via [CryptoJS](https://code.google.com/archive/p/crypto-js/)

## Usage
Pastebin builds only with `rust-nightly` version and requires `llvm` compiler to be present (rocksdb deps). To skip the build process, you can use the docker image.

### Cargo
```
cargo build --release
cargo run
```
### Docker
```
docker pull mkaczanowski/pastebin:latest
docker run mkaczanowski/pastebin --address localhost --port 8000
```

### Client
```
alias pastebin="curl -q -L -d @- -o - http://localhost:8000/"

echo "hello World" | pastebin
http://localhost:8000/T9kGrI5aNkI4Z-PelmQ5U
```

## Nginx (optional)
The Pastebin service serves `/static` files from memory. To lower down the load on the service you might want to consider setting up nginx with caching and compression enabled, as shown here:
```
map $sent_http_content_type $expires {
    default                    off;
    text/css                   30d;
    application/javascript     30d;
    image/x-icon               30d;
}

server {
    listen       80; 
    server_name  paste.domain.com;
    
    gzip on;
    gzip_types text/plain application/xml text/css application/javascript;

    expires $expires;
    location  / {
        proxy_pass        http://localhost:8000;
           include        proxy-settings.conf;
    }

    access_log /var/log/nginx/access.log;
}
```

## Demo
[![Pastebin service demo](https://i.imgur.com/Fv19H71.png)](https://www.youtube.com/watch?v=BG7f61H7C4I "Pastebin service demo")
