[![Build Status][travis-badge]][travis]
[![Test and Build][github-workflow]][github-workflow]
[![Docker Cloud Build Status][docker-cloud-build-status]][docker-hub]
[![Docker Pulls][docker-pulls]][docker-hub]
[![Docker Image Size][docker-size]][docker-hub]

[travis-badge]: https://travis-ci.org/mkaczanowski/pastebin.svg?branch=master
[travis]: https://travis-ci.org/mkaczanowski/pastebin/
[docker-hub]: https://hub.docker.com/r/mkaczanowski/pastebin
[docker-cloud-build-status]: https://img.shields.io/docker/cloud/build/mkaczanowski/pastebin
[docker-pulls]: https://img.shields.io/docker/pulls/mkaczanowski/pastebin
[docker-size]: https://img.shields.io/docker/image-size/mkaczanowski/pastebin
[github-workflow]: https://github.com/mkaczanowski/pastebin/workflows/Test%20and%20Build/badge.svg

# Pastebin
Simple, fast, standalone pastebin service.

## Why?
Whenever you need to share a code snippet, diff, logs, or a secret with another human being, the Pastebin service is invaluable. However, using public services such as [pastebin.com](https://pastebin.com), [privnote.com](https://privnote.com), etc. should be avoided when you're sharing data that should be available only for a selected audience (i.e., your company, private network). Instead of trusting external providers, you could host your own Pastebin service and take ownership of all your data!

**There are numerous [Pastebin implementations](https://github.com/awesome-selfhosted/awesome-selfhosted#pastebins) out there, why would you implement another one?**

While the other implementations are great, I couldn't find one that would satisfy my requirements:
* no dependencies - one binary is all I want, no python libs, ruby runtime magic, no javascript or external databases to setup
* storage - fast, lightweight, self-hosted key-value storage able to hold a lot of data.
* speed - it must be fast. Once deployed in a mid-sized company you can expect high(er) traffic with low latency expectations from users
* reliability - no one wants to fix things that should just work (and are that simple!)
* cheap - low-cost service that would not steal too much CPU time, thus adding up to your bill
* CLI + GUI - it must be easy to interface from both ends (but still, no deps!)
* other features:
    * on-demand encryption
    * syntax highlighting
    * destroy after reading
    * destroy after expiration date

This Pastebin implementation satisfies all of the above requirements!

## Implementation
This is a rust version of Pastebin service with [rocksdb](https://rocksdb.org/) database as storage. In addition to previously mentioned features it's worth to mention:
* all-in-one binary - all the data, including css/javascript files are compiled into the binary. This way you don't need to worry about external dependencies, it's all within. (see: [std::include_bytes](https://doc.rust-lang.org/std/macro.include_bytes.html))
* [REST endpoint](https://rocket.rs/) - you can add/delete pastes via standard HTTP client (ie. curl)
* [RocksDB compaction filter](https://github.com/facebook/rocksdb/wiki/Compaction-Filter) - the expired pastes will be automatically removed by custom compaction filter
* [flatbuffers](https://google.github.io/flatbuffers/) - data is serialized with flatbuffers (access to serialized data without parsing/unpacking)
* GUI - the UI is a plain HTML with [Bootstrap JS](https://getbootstrap.com/), [jQuery](https://jquery.com/) and [prism.js](https://prismjs.com/)
* Encryption - password-protected pastes are AES encrypted/decprypted in the browser via [CryptoJS](https://code.google.com/archive/p/crypto-js/)

### Plugins
The default configuration enables only one plugin, this is syntax highlighting through `prism.js`. This should be enough for p90 of the users but if you need extra features you might want to use the plugin system (`src/plugins`).

To enable additional plugins, pass:
```
--plugins prism <custom_plugin_name>
```

Currently supported:
* [prism.js](https://prismjs.com/)
* [mermaid.js](https://github.com/mermaid-js/mermaid)


## Usage
Pastebin builds only with `rust-nightly` version and requires `llvm` compiler (rocksdb deps). To skip the build process, you can use the docker image.

### Cargo
```
cargo build --release
cargo run
```
### Docker
```
docker pull mkaczanowski/pastebin:latest
docker run --network host mkaczanowski/pastebin --address localhost --port 8000
```

### Client
```
alias pastebin="curl -w '\n' -q -L --data-binary @- -o - http://localhost:8000/"

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

## Benchmark
I used [k6.io](https://k6.io/) for benchmarking the read-by-id HTTP endoint. Details:
* CPU: Intel(R) Core(TM) i7-8650U CPU @ 1.90GHz (4 CPUs, 8 threads = 16 rocket workers)
* Mem: 24 GiB
* Storage: NVMe SSD Controller SM981/PM981/PM983
* both client (k6) and server (pastebin) running on the same machine

### Setup
```
$ cargo run --release

$ echo "Hello world" | curl -q -L -d @- -o - http://localhost:8000/
http://localhost:8000/0FWc4aaZXzf6GZBsuW4nv

$ cat > script.js <<EOL
import http from "k6/http";

export default function() {
    let response = http.get("http://localhost:8000/<ID>");
};
EOL

$ docker pull loadimpact/k6
```

### Test 1: 5 concurrent clients, duration: 15s
```
$ docker run --network=host -i loadimpact/k6 run --vus 5 -d 15s - <script.js

data_received..............: 206 MB 14 MB/s
data_sent..................: 1.6 MB 108 kB/s
http_req_blocked...........: avg=203.98µs min=59.63µs med=97.34µs max=280.74ms p(90)=142.01µs p(95)=161.72µs
http_req_connecting........: avg=60.48µs  min=0s      med=54.22µs max=9.57ms   p(90)=79.67µs  p(95)=93.6µs  
http_req_duration..........: avg=4.75ms   min=2.87ms  med=4.66ms  max=27.25ms  p(90)=6.02ms   p(95)=6.59ms  
http_req_receiving.........: avg=69.16µs  min=18.54µs med=59µs    max=12.94ms  p(90)=103µs    p(95)=128.14µs
http_req_sending...........: avg=53.21µs  min=18.11µs med=33.01µs max=5.82ms   p(90)=62.68µs  p(95)=166.06µs
http_req_tls_handshaking...: avg=0s       min=0s      med=0s      max=0s       p(90)=0s       p(95)=0s      
http_req_waiting...........: avg=4.62ms   min=2.8ms   med=4.54ms  max=20.25ms  p(90)=5.87ms   p(95)=6.36ms  
http_reqs..................: 14986  999.062363/s
iteration_duration.........: avg=4.98ms   min=2.96ms  med=4.8ms   max=299.92ms p(90)=6.18ms   p(95)=6.77ms  
iterations.................: 14986  999.062363/s
vus........................: 5      min=5 max=5
vus_max....................: 5      min=5 max=5
```

### Test 2: Every 15s double concurrent clients
```
docker run --network=host -i loadimpact/k6 run --vus 2 --stage 15s:4,15s:8,15s:16,15s:32 - <script.js

data_received..............: 654 MB 11 MB/s
data_sent..................: 5.9 MB 98 kB/s
http_req_blocked...........: avg=175.61µs min=56.88µs med=133.4µs max=168.74ms p(90)=175.38µs p(95)=219.87µs
http_req_connecting........: avg=86.58µs  min=0s      med=67.93µs max=34.36ms  p(90)=95.52µs  p(95)=116.89µs
http_req_duration..........: avg=13.29ms  min=2.64ms  med=8.3ms   max=129.12ms p(90)=30.32ms  p(95)=38.67ms 
http_req_receiving.........: avg=223.36µs min=18.63µs med=71.91µs max=39.84ms  p(90)=143.88µs p(95)=217.81µs
http_req_sending...........: avg=461.61µs min=17.23µs med=46.8µs  max=62.26ms  p(90)=335.01µs p(95)=857.64µs
http_req_tls_handshaking...: avg=0s       min=0s      med=0s      max=0s       p(90)=0s       p(95)=0s      
http_req_waiting...........: avg=12.6ms   min=2.59ms  med=8ms     max=106.26ms p(90)=28.61ms  p(95)=36.55ms 
http_reqs..................: 47699  794.982442/s
iteration_duration.........: avg=13.48ms  min=2.75ms  med=8.47ms  max=185.95ms p(90)=30.55ms  p(95)=38.91ms 
iterations.................: 47699  794.982442/s
vus........................: 31     min=2  max=31
vus_max....................: 32     min=32 max=32
```

### Interpretation
At first glance, the performance is pretty good. In the simplest scenario (5 concurrent clients), we can get up to `1000 rps` with the p95 response time at `6.59 ms` (`14986` total requests made).

As we add more concurrent clients, the rps drops a bit (`794 rps`) but still provides a good timing (p95 `38.67ms`) with high throughput at `47699` request made in 15s window (3x compared to Test 1).

The CPU utilization is at 100% on every core available and the memory usage is stable at `~13 Mb RSS`.

## Demo
[![Pastebin service demo](https://i.imgur.com/Fv19H71.png)](https://www.youtube.com/watch?v=BG7f61H7C4I "Pastebin service demo")
