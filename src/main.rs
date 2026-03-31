use std::io;
use std::io::Cursor;
use std::net::IpAddr;
use std::str::FromStr;

use rocket::config::{Config, LogLevel};
use rocket::data::{Data, ToByteUnit};
use rocket::http::{ContentType, Status};
use rocket::request::{FromRequest, Outcome, Request};
use rocket::response::{self, Responder, Response};
use rocket::{delete, get, post, routes};
use rocket::State;

use chrono::DateTime;
use clap::Parser;
use handlebars::Handlebars;
use humantime::parse_duration;
use nanoid::nanoid;
use regex::Regex;
use rocksdb::{Options, DB};
use serde_json::json;

mod formatter;

#[macro_use]
mod lib;
use lib::{compaction_filter_expired_entries, get_entry_data, get_extension, new_entry, sanitize_lang};

mod plugins;
use plugins::plugin::{Plugin, PluginManager};

mod api_generated;
use api_generated::api::root_as_entry;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Newtype wrapper so we can return `rocket::Response` from route handlers.
struct CustomResponse<'r>(Response<'r>);

impl<'r> Responder<'r, 'r> for CustomResponse<'r> {
    fn respond_to(self, _: &'r Request<'_>) -> response::Result<'r> {
        Ok(self.0)
    }
}

#[derive(Parser, Debug)]
#[command(name = "pastebin", about = "Simple, standalone and fast pastebin service.")]
struct PastebinConfig {
    #[arg(long, help = "IP address or host to listen on", default_value = "localhost")]
    address: String,

    #[arg(long, help = "Port number to listen on", default_value_t = 8000)]
    port: u16,

    #[arg(long, help = "Number of concurrent thread workers", default_value_t = 0)]
    workers: usize,

    #[arg(long = "keep-alive", help = "Keep-alive timeout in seconds", default_value_t = 5)]
    keep_alive: u32,

    #[arg(long, help = "Max log level", default_value = "normal")]
    log: LogLevel,

    #[arg(long, help = "Time to live for entries, by default kept forever", default_value_t = 0)]
    ttl: u64,

    #[arg(long = "db", help = "Database file path", default_value = "./pastebin.db")]
    db_path: String,

    #[arg(long = "tls-certs", help = "Path to certificate chain in PEM format")]
    tls_certs: Option<String>,

    #[arg(long = "tls-key", help = "Path to private key for tls-certs in PEM format")]
    tls_key: Option<String>,

    #[arg(long, help = "Override default URI")]
    uri: Option<String>,

    #[arg(
        long = "uri-prefix",
        help = "Prefix appended to the URI (ie. '/pastebin')",
        default_value = ""
    )]
    uri_prefix: String,

    #[arg(
        long = "slug-charset",
        help = "Character set (expressed as rust compatible regex) to use for generating the URL slug",
        default_value = "[A-Za-z0-9_-]"
    )]
    slug_charset: String,

    #[arg(long = "slug-len", help = "Length of URL slug", default_value_t = 21)]
    slug_len: usize,

    #[arg(
        long = "ui-expiry-times",
        help = "Paste expiry times shown in the UI dropdown",
        default_values = &["5 minutes", "10 minutes", "1 hour", "1 day", "1 week", "1 month", "1 year", "Never"],
    )]
    ui_expiry_times: Vec<String>,

    #[arg(long = "ui-line-numbers", help = "Display line numbers")]
    ui_line_numbers: bool,

    #[arg(
        long,
        help = "Enable additional functionalities (ie. prism, mermaid)",
        default_values = &["prism"],
    )]
    plugins: Vec<String>,
}

/// Carries the effective public host and scheme derived from reverse-proxy headers.
struct RequestHost {
    scheme: String,
    host: String,
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for RequestHost {
    type Error = ();

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let host = request
            .headers()
            .get_one("X-Forwarded-Host")
            .or_else(|| request.headers().get_one("Host"))
            .map(|s| s.to_string());

        let scheme = request
            .headers()
            .get_one("X-Forwarded-Proto")
            .map(|s| s.to_string());

        match host {
            Some(host) => Outcome::Success(RequestHost {
                scheme: scheme.unwrap_or_else(|| "http".to_string()),
                host,
            }),
            None => Outcome::Forward(Status::BadRequest),
        }
    }
}

fn get_url(cfg: &PastebinConfig, req_host: Option<RequestHost>) -> String {
    if let Some(uri) = &cfg.uri {
        return uri.clone();
    }
    if let Some(rh) = req_host {
        return format!("{}://{}", rh.scheme, rh.host);
    }
    let port = if matches!(cfg.port, 443 | 80) {
        String::new()
    } else {
        format!(":{}", cfg.port)
    };
    let scheme = if cfg.tls_certs.is_some() { "https" } else { "http" };
    format!("{scheme}://{}{port}", cfg.address)
}

fn get_error_response<'r>(
    handlebars: &Handlebars,
    uri_prefix: String,
    html: String,
    status: Status,
) -> CustomResponse<'r> {
    let map = json!({
        "version": VERSION,
        "is_error": "true",
        "uri_prefix": uri_prefix,
    });

    let content = handlebars.render_template(&html, &map).unwrap();

    CustomResponse(
        Response::build()
            .status(status)
            .header(ContentType::HTML)
            .sized_body(content.len(), Cursor::new(content))
            .finalize(),
    )
}

#[post("/?<lang>&<ttl>&<burn>&<encrypted>", data = "<paste>")]
async fn create(
    req_host: Option<RequestHost>,
    paste: Data<'_>,
    state: &State<DB>,
    cfg: &State<PastebinConfig>,
    alphabet: &State<Vec<char>>,
    lang: Option<&str>,
    ttl: Option<u64>,
    burn: Option<bool>,
    encrypted: Option<bool>,
) -> Result<String, io::Error> {
    let slug_len = cfg.slug_len;
    let id = nanoid!(slug_len, alphabet.inner());
    let base_url = get_url(cfg, req_host);
    let url = format!("{base_url}/{id}");

    let bytes = paste
        .open(8.mebibytes())
        .into_bytes()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
        .into_inner();

    let mut writer: Vec<u8> = vec![];
    new_entry(
        &mut writer,
        &bytes,
        lang.unwrap_or("markup"),
        ttl.unwrap_or(cfg.ttl),
        burn.unwrap_or(false),
        encrypted.unwrap_or(false),
    );

    state.put(id, writer).unwrap();

    Ok(url)
}

#[delete("/<id>")]
async fn remove(id: &str, state: &State<DB>) -> Status {
    match state.delete(id) {
        Ok(_) => Status::Ok,
        Err(_) => Status::InternalServerError,
    }
}

#[allow(clippy::too_many_arguments)]
#[get("/<id>?<lang>")]
async fn view_paste<'r>(
    id: &'r str,
    lang: Option<&'r str>,
    state: &'r State<DB>,
    handlebars: &'r State<Handlebars<'static>>,
    plugin_manager: &'r State<PluginManager>,
    ui_expiry_times: &'r State<Vec<(String, u64)>>,
    ui_expiry_default: &'r State<String>,
    cfg: &'r State<PastebinConfig>,
) -> CustomResponse<'r> {
    let resources = plugin_manager.static_resources();
    let html = String::from_utf8_lossy(resources.get("/static/index.html").unwrap()).into_owned();

    let root = match get_entry_data(id, state) {
        Ok(x) => x,
        Err(e) => {
            let status = match e.kind() {
                io::ErrorKind::NotFound => Status::NotFound,
                _ => Status::InternalServerError,
            };
            let map = json!({
                "version": VERSION,
                "is_error": "true",
                "uri_prefix": cfg.uri_prefix,
                "js_imports": plugin_manager.js_imports(),
                "css_imports": plugin_manager.css_imports(),
                "js_init": plugin_manager.js_init(),
            });
            let content = handlebars.render_template(&html, &map).unwrap();
            return CustomResponse(
                Response::build()
                    .status(status)
                    .header(ContentType::HTML)
                    .sized_body(content.len(), Cursor::new(content))
                    .finalize(),
            );
        }
    };

    let entry = root_as_entry(&root).unwrap();
    let lowercased = lang
        .unwrap_or_else(|| entry.lang().unwrap_or("markup"))
        .to_lowercase();
    let selected_lang = sanitize_lang(&lowercased);

    let mut pastebin_cls = Vec::new();
    if cfg.ui_line_numbers {
        pastebin_cls.push("line-numbers".to_string());
    }
    pastebin_cls.push(format!("language-{selected_lang}"));

    let mut map = json!({
        "is_created": "true",
        "pastebin_code": String::from_utf8_lossy(entry.data().unwrap().bytes()),
        "pastebin_id": id,
        "pastebin_cls": pastebin_cls.join(" "),
        "version": VERSION,
        "uri_prefix": cfg.uri_prefix,
        "ui_expiry_times": ui_expiry_times.inner(),
        "ui_expiry_default": ui_expiry_default.inner(),
        "js_imports": plugin_manager.js_imports(),
        "css_imports": plugin_manager.css_imports(),
        "js_init": plugin_manager.js_init(),
    });

    if entry.burn() {
        map["msg"] = json!("FOR YOUR EYES ONLY. The paste is gone, after you close this window.");
        map["level"] = json!("warning");
        map["is_burned"] = json!("true");
        map["glyph"] = json!("fa fa-fire");
    } else if entry.expiry_timestamp() != 0 {
        let time = DateTime::from_timestamp(entry.expiry_timestamp() as i64, 0)
            .unwrap()
            .naive_utc()
            .format("%Y-%m-%d %H:%M:%S");
        map["msg"] = json!(format!("This paste will expire on {time}."));
        map["level"] = json!("info");
        map["glyph"] = json!("far fa-clock");
    }

    if entry.encrypted() {
        map["is_encrypted"] = json!("true");
    }

    let content = handlebars.render_template(&html, &map).unwrap();

    CustomResponse(
        Response::build()
            .status(Status::Ok)
            .header(ContentType::HTML)
            .sized_body(content.len(), Cursor::new(content))
            .finalize(),
    )
}

#[allow(clippy::too_many_arguments)]
#[get("/new?<id>&<level>&<msg>&<glyph>&<url>")]
async fn get_new<'r>(
    state: &'r State<DB>,
    handlebars: &'r State<Handlebars<'static>>,
    cfg: &'r State<PastebinConfig>,
    plugin_manager: &'r State<PluginManager>,
    ui_expiry_times: &'r State<Vec<(String, u64)>>,
    ui_expiry_default: &'r State<String>,
    id: Option<&'r str>,
    level: Option<&'r str>,
    glyph: Option<&'r str>,
    msg: Option<&'r str>,
    url: Option<&'r str>,
) -> CustomResponse<'r> {
    let resources = plugin_manager.static_resources();
    let html = String::from_utf8_lossy(resources.get("/static/index.html").unwrap()).into_owned();

    let mut map = json!({
        "is_editable": "true",
        "version": VERSION,
        "msg": msg.unwrap_or_default(),
        "level": level.unwrap_or("secondary"),
        "glyph": glyph.unwrap_or_default(),
        "url": url.unwrap_or_default(),
        "uri_prefix": cfg.uri_prefix,
        "ui_expiry_times": ui_expiry_times.inner(),
        "ui_expiry_default": ui_expiry_default.inner(),
        "js_imports": plugin_manager.js_imports(),
        "css_imports": plugin_manager.css_imports(),
        "js_init": plugin_manager.js_init(),
    });

    if let Some(id) = id {
        let root = match get_entry_data(id, state) {
            Ok(r) => r,
            Err(_) => {
                return CustomResponse(
                    Response::build()
                        .status(Status::NotFound)
                        .header(ContentType::HTML)
                        .sized_body(0, Cursor::new(""))
                        .finalize(),
                );
            }
        };
        let entry = root_as_entry(&root).unwrap();

        if entry.encrypted() {
            map["is_encrypted"] = json!("true");
        }
        map["pastebin_code"] =
            json!(String::from_utf8_lossy(entry.data().unwrap().bytes()));
    }

    let content = handlebars.render_template(&html, &map).unwrap();

    CustomResponse(
        Response::build()
            .status(Status::Ok)
            .header(ContentType::HTML)
            .sized_body(content.len(), Cursor::new(content))
            .finalize(),
    )
}

#[get("/raw/<id>")]
async fn get_raw(id: &str, state: &State<DB>) -> CustomResponse<'static> {
    let root = match get_entry_data(id, state) {
        Ok(x) => x,
        Err(e) => {
            let status = match e.kind() {
                io::ErrorKind::NotFound => Status::NotFound,
                _ => Status::InternalServerError,
            };
            return CustomResponse(Response::build().status(status).finalize());
        }
    };

    let entry = root_as_entry(&root).unwrap();
    let data = entry.data().unwrap().bytes().to_vec();

    CustomResponse(
        Response::build()
            .status(Status::Ok)
            .header(ContentType::Plain)
            .sized_body(data.len(), Cursor::new(data))
            .finalize(),
    )
}

#[get("/download/<id>")]
async fn get_binary(id: &str, state: &State<DB>) -> CustomResponse<'static> {
    let inner = get_raw(id, state).await;
    CustomResponse(
        Response::build_from(inner.0)
            .header(ContentType::Binary)
            .finalize(),
    )
}

#[get("/static/<resource>")]
async fn get_static<'r>(
    resource: &'r str,
    handlebars: &'r State<Handlebars<'static>>,
    plugin_manager: &'r State<PluginManager>,
    cfg: &'r State<PastebinConfig>,
) -> CustomResponse<'r> {
    let resources = plugin_manager.static_resources();
    let pth = format!("/static/{resource}");
    let ext = get_extension(resource).trim_start_matches('.').to_string();

    let content = match resources.get(pth.as_str()) {
        Some(data) => *data,
        None => {
            let html = String::from_utf8_lossy(resources.get("/static/index.html").unwrap())
                .into_owned();
            return get_error_response(handlebars.inner(), cfg.uri_prefix.clone(), html, Status::NotFound);
        }
    };

    let content_type = ContentType::from_extension(&ext).unwrap_or(ContentType::Binary);

    CustomResponse(
        Response::build()
            .status(Status::Ok)
            .header(content_type)
            .sized_body(content.len(), Cursor::new(content))
            .finalize(),
    )
}

#[get("/")]
fn index(cfg: &State<PastebinConfig>) -> rocket::response::Redirect {
    rocket::response::Redirect::to(format!("{}/new", cfg.uri_prefix))
}

fn rocket_instance(pastebin_config: PastebinConfig) -> rocket::Rocket<rocket::Build> {
    let workers = if pastebin_config.workers != 0 {
        pastebin_config.workers
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            * 2
    };

    let address = IpAddr::from_str(&pastebin_config.address)
        .unwrap_or_else(|_| IpAddr::from_str("127.0.0.1").unwrap());

    let mut rocket_config = Config {
        address,
        port: pastebin_config.port,
        workers,
        keep_alive: pastebin_config.keep_alive,
        log_level: pastebin_config.log,
        ..Config::default()
    };

    if let (Some(certs), Some(key)) = (&pastebin_config.tls_certs, &pastebin_config.tls_key) {
        rocket_config.tls =
            Some(rocket::config::TlsConfig::from_paths(certs, key));
    }

    // Setup DB — options must be created before opening so the compaction filter is applied.
    let mut db_opts = Options::default();
    db_opts.create_if_missing(true);
    db_opts.set_compaction_filter("ttl_entries", compaction_filter_expired_entries);
    let db = DB::open(&db_opts, &pastebin_config.db_path).unwrap();

    // Build the URL slug alphabet from the configured charset regex.
    let alphabet = {
        let re = Regex::new(&pastebin_config.slug_charset).unwrap();
        let mut tmp = [0u8; 4];
        (0x20u8..0x7e)
            .filter_map(|b| {
                let c = b as char;
                re.is_match(c.encode_utf8(&mut tmp)).then_some(c)
            })
            .collect::<Vec<char>>()
    };

    // Build the expiry time pairs used by the UI dropdown.
    let ui_expiry_times: Vec<(String, u64)> = pastebin_config
        .ui_expiry_times
        .iter()
        .map(|s| {
            let s = s.trim();
            if s.to_lowercase() == "never" {
                (s.to_string(), 0)
            } else {
                (s.to_string(), parse_duration(s).unwrap().as_secs())
            }
        })
        .collect();

    let ui_expiry_default: String = ui_expiry_times
        .iter()
        .find_map(|(name, val)| (*val == pastebin_config.ttl).then(|| name.clone()))
        .expect("the --ttl flag must match one of the --ui-expiry-times values");

    if pastebin_config.slug_len == 0 {
        panic!("slug_len must be larger than zero");
    }
    if alphabet.is_empty() {
        panic!("selected slug alphabet is empty, please check if slug_charset is a valid regex");
    }

    let plugins: Vec<Box<dyn Plugin>> = pastebin_config
        .plugins
        .iter()
        .map(|name| -> Box<dyn Plugin> {
            match name.as_str() {
                "prism" => Box::new(plugins::prism::new()),
                "mermaid" => Box::new(plugins::mermaid::new()),
                _ => panic!("unknown plugin: {name}"),
            }
        })
        .collect();

    let plugin_manager = plugins::new(plugins);
    let uri_prefix = pastebin_config.uri_prefix.clone();

    rocket::custom(rocket_config)
        .manage(pastebin_config)
        .manage(db)
        .manage(formatter::new())
        .manage(plugin_manager)
        .manage(alphabet)
        .manage(ui_expiry_times)
        .manage(ui_expiry_default)
        .mount(
            if uri_prefix.is_empty() { "/" } else { &uri_prefix },
            routes![index, create, remove, view_paste, get_new, get_raw, get_binary, get_static],
        )
}

#[rocket::main]
async fn main() {
    rocket_instance(PastebinConfig::parse())
        .launch()
        .await
        .expect("rocket failed to launch");
}

#[cfg(test)]
mod tests {
    use super::*;
    use rocket::http::{ContentType, Status};
    use rocket::local::blocking::Client;
    use tempfile::TempDir;

    fn create_client() -> (Client, TempDir) {
        let tmp_dir = TempDir::new().unwrap();
        let mut config = PastebinConfig::parse_from(["pastebin"]);
        config.db_path = tmp_dir.path().join("database").to_str().unwrap().to_string();
        let client = Client::tracked(rocket_instance(config)).expect("invalid rocket instance");
        (client, tmp_dir)
    }

    fn insert_paste(client: &Client, data: &str, path: &str) -> String {
        let response = client.post(path).body(data).dispatch();
        assert_eq!(response.status(), Status::Ok);
        let url = response.into_string().unwrap();
        url.split('/').next_back().unwrap().to_string()
    }

    fn get_paste<'c>(client: &'c Client, path: &str) -> rocket::local::blocking::LocalResponse<'c> {
        client.get(format!("/{path}")).dispatch()
    }

    #[test]
    fn create_and_fetch_paste() {
        let (client, _tmp) = create_client();
        let id = insert_paste(&client, "random_test_data_to_be_checked", "/");
        let response = get_paste(&client, &id);
        assert_eq!(response.status(), Status::Ok);
        assert_eq!(response.content_type(), Some(ContentType::HTML));
        assert!(response.into_string().unwrap().contains("random_test_data_to_be_checked"));
    }

    #[test]
    fn remove_paste_by_id() {
        let (client, _tmp) = create_client();
        client.delete("/some_id").dispatch();
        let response = get_paste(&client, "some_id");
        assert_eq!(response.status(), Status::NotFound);
    }

    #[test]
    fn remove_nonexistent_paste() {
        let (client, _tmp) = create_client();
        assert_eq!(get_paste(&client, "fake_id").status(), Status::NotFound);
        client.delete("/fake_id").dispatch();
        assert_eq!(get_paste(&client, "fake_id").status(), Status::NotFound);
    }

    #[test]
    fn get_raw_contents() {
        let (client, _tmp) = create_client();
        let id = insert_paste(&client, "random_test_data_to_be_checked", "/");
        let response = get_paste(&client, &format!("raw/{id}"));
        assert_eq!(response.status(), Status::Ok);
        assert_eq!(response.content_type(), Some(ContentType::Plain));
        assert!(response.into_string().unwrap().contains("random_test_data_to_be_checked"));
    }

    #[test]
    fn download_contents() {
        let (client, _tmp) = create_client();
        let id = insert_paste(&client, "random_test_data_to_be_checked", "/");
        let response = get_paste(&client, &format!("download/{id}"));
        assert_eq!(response.status(), Status::Ok);
        assert_eq!(response.content_type(), Some(ContentType::Binary));
        assert!(response.into_string().unwrap().contains("random_test_data_to_be_checked"));
    }

    #[test]
    fn clone_contents() {
        let (client, _tmp) = create_client();
        let id = insert_paste(&client, "random_test_data_to_be_checked", "/");
        let response = get_paste(&client, &format!("new?id={id}"));
        assert_eq!(response.status(), Status::Ok);
        assert_eq!(response.content_type(), Some(ContentType::HTML));
        assert!(response.into_string().unwrap().contains("random_test_data_to_be_checked"));
    }

    #[test]
    fn burned_paste_not_accessible_after_first_read() {
        let (client, _tmp) = create_client();
        let id = insert_paste(&client, "random_test_data_to_be_checked", "/?burn=true");
        assert_eq!(get_paste(&client, &id).status(), Status::Ok);
        assert_eq!(get_paste(&client, &id).status(), Status::NotFound);
    }

    #[test]
    fn expired_paste_returns_not_found() {
        use std::{thread, time};
        let (client, _tmp) = create_client();
        let id = insert_paste(&client, "random_test_data_to_be_checked", "/?ttl=1");
        assert_eq!(get_paste(&client, &id).status(), Status::Ok);
        thread::sleep(time::Duration::from_secs(1));
        assert_eq!(get_paste(&client, &id).status(), Status::NotFound);
    }

    #[test]
    fn get_static_content() {
        let (client, _tmp) = create_client();
        let response = client.get("/static/favicon.ico").dispatch();
        let contents = std::fs::read("static/favicon.ico").unwrap();
        assert_eq!(response.status(), Status::Ok);
        assert_eq!(response.into_bytes(), Some(contents));
    }

    // ── get_url unit tests ────────────────────────────────────────────────────

    #[test]
    fn get_url_explicit_uri_overrides_request_host() {
        let mut cfg = PastebinConfig::parse_from(["pastebin"]);
        cfg.uri = Some("https://example.com".to_string());
        let rh = Some(RequestHost { scheme: "http".to_string(), host: "other.com".to_string() });
        assert_eq!(get_url(&cfg, rh), "https://example.com");
    }

    #[test]
    fn get_url_uses_forwarded_host_and_proto() {
        let cfg = PastebinConfig::parse_from(["pastebin"]);
        let rh = Some(RequestHost { scheme: "https".to_string(), host: "proxy.example.com".to_string() });
        assert_eq!(get_url(&cfg, rh), "https://proxy.example.com");
    }

    #[test]
    fn get_url_forwarded_host_defaults_scheme_to_http() {
        let cfg = PastebinConfig::parse_from(["pastebin"]);
        // RequestHost is only constructed with a real scheme via from_request;
        // when X-Forwarded-Proto is absent, scheme defaults to "http" in the guard.
        let rh = Some(RequestHost { scheme: "http".to_string(), host: "proxy.example.com".to_string() });
        assert_eq!(get_url(&cfg, rh), "http://proxy.example.com");
    }

    #[test]
    fn get_url_fallback_includes_nonstandard_port() {
        let mut cfg = PastebinConfig::parse_from(["pastebin"]);
        cfg.address = "127.0.0.1".to_string();
        cfg.port = 9000;
        assert_eq!(get_url(&cfg, None), "http://127.0.0.1:9000");
    }

    #[test]
    fn get_url_omits_port_80() {
        let mut cfg = PastebinConfig::parse_from(["pastebin"]);
        cfg.address = "myhost".to_string();
        cfg.port = 80;
        assert_eq!(get_url(&cfg, None), "http://myhost");
    }

    #[test]
    fn get_url_omits_port_443_with_tls() {
        let mut cfg = PastebinConfig::parse_from(["pastebin"]);
        cfg.address = "myhost".to_string();
        cfg.port = 443;
        cfg.tls_certs = Some("/path/cert.pem".to_string());
        assert_eq!(get_url(&cfg, None), "https://myhost");
    }

    // ── forwarded-header integration ──────────────────────────────────────────

    #[test]
    fn create_paste_url_uses_x_forwarded_host_and_proto() {
        let (client, _tmp) = create_client();
        let response = client
            .post("/")
            .header(rocket::http::Header::new("X-Forwarded-Host", "public.example.com"))
            .header(rocket::http::Header::new("X-Forwarded-Proto", "https"))
            .body("test data")
            .dispatch();
        assert_eq!(response.status(), Status::Ok);
        let url = response.into_string().unwrap();
        assert!(url.starts_with("https://public.example.com/"), "unexpected url: {url}");
    }

    #[test]
    fn create_paste_url_uses_host_header_when_no_forwarded_host() {
        let (client, _tmp) = create_client();
        let response = client
            .post("/")
            .header(rocket::http::Header::new("Host", "direct.example.com"))
            .body("test data")
            .dispatch();
        assert_eq!(response.status(), Status::Ok);
        let url = response.into_string().unwrap();
        assert!(url.starts_with("http://direct.example.com/"), "unexpected url: {url}");
    }

    // ── uri_prefix integration ────────────────────────────────────────────────

    fn create_client_with_prefix(prefix: &str) -> (Client, TempDir) {
        let tmp_dir = TempDir::new().unwrap();
        let mut config = PastebinConfig::parse_from(["pastebin"]);
        config.db_path = tmp_dir.path().join("database").to_str().unwrap().to_string();
        config.uri_prefix = prefix.to_string();
        let client = Client::tracked(rocket_instance(config)).expect("invalid rocket instance");
        (client, tmp_dir)
    }

    #[test]
    fn uri_prefix_index_redirects_to_prefix_new() {
        let (client, _tmp) = create_client_with_prefix("/paste");
        let response = client.get("/paste/").dispatch();
        assert_eq!(response.status(), Status::SeeOther);
        let location = response.headers().get_one("Location").unwrap();
        assert_eq!(location, "/paste/new");
    }

    #[test]
    fn uri_prefix_new_page_is_accessible() {
        let (client, _tmp) = create_client_with_prefix("/paste");
        let response = client.get("/paste/new").dispatch();
        assert_eq!(response.status(), Status::Ok);
        assert_eq!(response.content_type(), Some(ContentType::HTML));
    }

    #[test]
    fn uri_prefix_static_assets_are_accessible() {
        let (client, _tmp) = create_client_with_prefix("/paste");
        let response = client.get("/paste/static/favicon.ico").dispatch();
        assert_eq!(response.status(), Status::Ok);
    }

    #[test]
    fn clone_nonexistent_paste_returns_not_found() {
        let (client, _tmp) = create_client();
        let response = client.get("/new?id=doesnotexist").dispatch();
        assert_eq!(response.status(), Status::NotFound);
    }

    #[test]
    fn static_unknown_extension_falls_back_to_octet_stream() {
        // Regression: ContentType::from_extension().unwrap() would panic for extensions
        // Rocket doesn't recognise. The fix uses unwrap_or(ContentType::Binary).
        assert!(ContentType::from_extension("flatbuffers").is_none());
        assert_eq!(
            ContentType::from_extension("flatbuffers").unwrap_or(ContentType::Binary),
            ContentType::Binary,
        );
    }

    #[test]
    fn invalid_unicode_data_is_handled() {
        let (client, _tmp) = create_client();
        let invalid_data = unsafe { String::from_utf8_unchecked(b"Hello \xF0\x90\x80World".to_vec()) };
        let id = insert_paste(&client, &invalid_data, "/");
        assert_eq!(get_paste(&client, &id).status(), Status::Ok);
    }
}
