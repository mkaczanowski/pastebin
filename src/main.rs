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
use lib::{compaction_filter_expired_entries, get_entry_data, get_extension, new_entry};

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
    lang: Option<String>,
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
        lang.unwrap_or_else(|| "markup".to_string()),
        ttl.unwrap_or(cfg.ttl),
        burn.unwrap_or(false),
        encrypted.unwrap_or(false),
    );

    state.put(id, writer).unwrap();

    Ok(url)
}

#[delete("/<id>")]
async fn remove(id: String, state: &State<DB>) -> Status {
    match state.delete(id) {
        Ok(_) => Status::Ok,
        Err(_) => Status::InternalServerError,
    }
}

#[allow(clippy::too_many_arguments)]
#[get("/<id>?<lang>")]
async fn view_paste<'r>(
    id: String,
    lang: Option<String>,
    state: &'r State<DB>,
    handlebars: &'r State<Handlebars<'static>>,
    plugin_manager: &'r State<PluginManager>,
    ui_expiry_times: &'r State<Vec<(String, u64)>>,
    ui_expiry_default: &'r State<String>,
    cfg: &'r State<PastebinConfig>,
) -> CustomResponse<'r> {
    let resources = plugin_manager.static_resources();
    let html = String::from_utf8_lossy(resources.get("/static/index.html").unwrap()).into_owned();

    let root = match get_entry_data(&id, state) {
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
    let selected_lang = lang
        .unwrap_or_else(|| entry.lang().unwrap().to_string())
        .to_lowercase();

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
    id: Option<String>,
    level: Option<String>,
    glyph: Option<String>,
    msg: Option<String>,
    url: Option<String>,
) -> CustomResponse<'r> {
    let resources = plugin_manager.static_resources();
    let html = String::from_utf8_lossy(resources.get("/static/index.html").unwrap()).into_owned();

    let mut map = json!({
        "is_editable": "true",
        "version": VERSION,
        "msg": msg.unwrap_or_default(),
        "level": level.unwrap_or_else(|| "secondary".to_string()),
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
        let root = get_entry_data(&id, state).unwrap();
        let entry = root_as_entry(&root).unwrap();

        if entry.encrypted() {
            map["is_encrypted"] = json!("true");
        }
        map["pastebin_code"] =
            json!(std::str::from_utf8(entry.data().unwrap().bytes()).unwrap());
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
async fn get_raw(id: String, state: &State<DB>) -> CustomResponse<'static> {
    let root = match get_entry_data(&id, state) {
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
async fn get_binary(id: String, state: &State<DB>) -> CustomResponse<'static> {
    let inner = get_raw(id, state).await;
    CustomResponse(
        Response::build_from(inner.0)
            .header(ContentType::Binary)
            .finalize(),
    )
}

#[get("/static/<resource>")]
async fn get_static<'r>(
    resource: String,
    handlebars: &'r State<Handlebars<'static>>,
    plugin_manager: &'r State<PluginManager>,
    cfg: &'r State<PastebinConfig>,
) -> CustomResponse<'r> {
    let resources = plugin_manager.static_resources();
    let pth = format!("/static/{resource}");
    let ext = get_extension(&resource).trim_start_matches('.').to_string();

    let content = match resources.get(pth.as_str()) {
        Some(data) => *data,
        None => {
            let html = String::from_utf8_lossy(resources.get("/static/index.html").unwrap())
                .into_owned();
            return get_error_response(handlebars.inner(), cfg.uri_prefix.clone(), html, Status::NotFound);
        }
    };

    let content_type = ContentType::from_extension(&ext).unwrap();

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

    #[test]
    fn invalid_unicode_data_is_handled() {
        let (client, _tmp) = create_client();
        let invalid_data = unsafe { String::from_utf8_unchecked(b"Hello \xF0\x90\x80World".to_vec()) };
        let id = insert_paste(&client, &invalid_data, "/");
        assert_eq!(get_paste(&client, &id).status(), Status::Ok);
    }
}
