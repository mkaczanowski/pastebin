#![allow(clippy::too_many_arguments)]

#[macro_use]
extern crate rocket;

use std::io;
use std::io::Cursor;
use std::net::IpAddr;
use std::path::Path;
use std::str::FromStr;

use rocket::config::{Config, LogLevel};
use rocket::data::{Data, ToByteUnit};
use rocket::http::{ContentType, Status};
use rocket::request::{FromRequest, Outcome, Request};
use rocket::response::{self, Responder, Response};
use rocket::State;

use chrono::DateTime;
use handlebars::Handlebars;
use humantime::parse_duration;
use nanoid::nanoid;
use regex::Regex;
use rocksdb::{Options, DB};
use serde_json::json;
use speculate::speculate;
use structopt::StructOpt;

mod formatter;

#[macro_use]
mod lib;
use lib::{compaction_filter_expired_entries, get_entry_data, get_extension, new_entry};

mod plugins;
use plugins::plugin::{Plugin, PluginManager};

mod api_generated;
use api_generated::api::root_as_entry;

speculate! {
    use rocket::local::blocking::Client;
    use rocket::http::Status;

    before {
        use tempfile::TempDir;

        // setup temporary database
        let tmp_dir = TempDir::new().unwrap();
        let file_path = tmp_dir.path().join("database");
        let mut pastebin_config = PastebinConfig::from_args();
        pastebin_config.db_path = file_path.to_str().unwrap().to_string();
        let rocket = rocket_instance(pastebin_config);

        // init rocket client
        let client = Client::tracked(rocket).expect("invalid rocket instance");
    }

    #[allow(dead_code)]
    fn insert_data(client: &Client, data: &str, path: &str) -> String {
        let response = client.post(path)
            .body(data)
            .dispatch();
        assert_eq!(response.status(), Status::Ok);

        // retrieve paste ID
        let url = response.into_string().unwrap();
        let id = url.split('/').collect::<Vec<&str>>().last().cloned().unwrap();

        id.to_string()
    }

    #[allow(dead_code)]
    fn get_data(client: &Client, path: String) -> rocket::local::blocking::LocalResponse<'_> {
        client.get(format!("/{}", path)).dispatch()
    }

    it "can get create and fetch paste" {
        // store data via post request
        let id = insert_data(&client, "random_test_data_to_be_checked", "/");

        // retrieve the data via get request
        let response = get_data(&client, id);
        assert_eq!(response.status(), Status::Ok);
        assert_eq!(response.content_type(), Some(ContentType::HTML));
        assert!(response.into_string().unwrap().contains("random_test_data_to_be_checked"));
    }

    it "can remove paste by id" {
        let response = client.delete("/some_id").dispatch();
        assert_eq!(response.status(), Status::Ok);

        let response = get_data(&client, "some_id".to_string());
        assert_eq!(response.status(), Status::NotFound);
    }

    it "can remove non-existing paste" {
        let response = get_data(&client, "some_fake_id".to_string());
        assert_eq!(response.status(), Status::NotFound);

        let response = client.delete("/some_fake_id").dispatch();
        assert_eq!(response.status(), Status::Ok);

        let response = get_data(&client, "some_fake_id".to_string());
        assert_eq!(response.status(), Status::NotFound);
    }

    it "can get raw contents" {
        // store data via post request
        let id = insert_data(&client, "random_test_data_to_be_checked", "/");

        // retrieve the data via get request
        let response = get_data(&client, format!("raw/{}", id));
        assert_eq!(response.status(), Status::Ok);
        assert_eq!(response.content_type(), Some(ContentType::Plain));
        assert!(response.into_string().unwrap().contains("random_test_data_to_be_checked"));
    }

    it "can download contents" {
        // store data via post request
        let id = insert_data(&client, "random_test_data_to_be_checked", "/");

        // retrieve the data via get request
        let response = get_data(&client, format!("download/{}", id));
        assert_eq!(response.status(), Status::Ok);
        assert_eq!(response.content_type(), Some(ContentType::Binary));
        assert!(response.into_string().unwrap().contains("random_test_data_to_be_checked"));
    }

    it "can clone contents" {
        // store data via post request
        let id = insert_data(&client, "random_test_data_to_be_checked", "/");

        // retrieve the data via get request
        let response = get_data(&client, format!("new?id={}", id));
        assert_eq!(response.status(), Status::Ok);
        assert_eq!(response.content_type(), Some(ContentType::HTML));
        assert!(response.into_string().unwrap().contains("random_test_data_to_be_checked"));
    }

    it "can't get burned paste" {
        // store data via post request
        let id = insert_data(&client, "random_test_data_to_be_checked", "/?burn=true");
        let response = get_data(&client, id.clone());
        assert_eq!(response.status(), Status::Ok);

        // retrieve the data via get request
        let response = get_data(&client, id);
        assert_eq!(response.status(), Status::NotFound);
    }

    it "can't get expired paste" {
        use std::{thread, time};

        // store data via post request
        let id = insert_data(&client, "random_test_data_to_be_checked", "/?ttl=1");
        let response = get_data(&client, id.clone());
        assert_eq!(response.status(), Status::Ok);

        thread::sleep(time::Duration::from_secs(1));

        // retrieve the data via get request
        let response = get_data(&client, id);
        assert_eq!(response.status(), Status::NotFound);
    }

    it "can get static contents" {
        let response = client.get("/static/favicon.ico").dispatch();
        let contents = std::fs::read("static/favicon.ico").unwrap();

        assert_eq!(response.status(), Status::Ok);
        assert_eq!(response.into_bytes(), Some(contents));
    }

    it "can cope with invalid unicode data" {
        let invalid_data = unsafe {
            String::from_utf8_unchecked(b"Hello \xF0\x90\x80World".to_vec())
        };
        let id = insert_data(&client, &invalid_data, "/");

        let response = get_data(&client, id);
        assert_eq!(response.status(), Status::Ok);
    }
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Newtype wrapper so we can return `rocket::Response` from route handlers.
struct CustomResponse<'r>(Response<'r>);

impl<'r> Responder<'r, 'r> for CustomResponse<'r> {
    fn respond_to(self, _: &'r Request<'_>) -> response::Result<'r> {
        Ok(self.0)
    }
}

#[derive(StructOpt, Debug)]
#[structopt(
    name = "pastebin",
    about = "Simple, standalone and fast pastebin service."
)]
struct PastebinConfig {
    #[structopt(
        long = "address",
        help = "IP address or host to listen on",
        default_value = "localhost"
    )]
    address: String,

    #[structopt(
        long = "port",
        help = "Port number to listen on",
        default_value = "8000"
    )]
    port: u16,

    #[structopt(
        long = "workers",
        help = "Number of concurrent thread workers",
        default_value = "0"
    )]
    workers: usize,

    #[structopt(
        long = "keep-alive",
        help = "Keep-alive timeout in seconds",
        default_value = "5"
    )]
    keep_alive: u32,

    #[structopt(long = "log", help = "Max log level", default_value = "normal")]
    log: LogLevel,

    #[structopt(
        long = "ttl",
        help = "Time to live for entries, by default kept forever",
        default_value = "0"
    )]
    ttl: u64,

    #[structopt(
        long = "db",
        help = "Database file path",
        default_value = "./pastebin.db"
    )]
    db_path: String,

    #[structopt(long = "tls-certs", help = "Path to certificate chain in PEM format")]
    tls_certs: Option<String>,

    #[structopt(
        long = "tls-key",
        help = "Path to private key for tls-certs in PEM format"
    )]
    tls_key: Option<String>,

    #[structopt(long = "uri", help = "Override default URI")]
    uri: Option<String>,

    #[structopt(
        long = "uri-prefix",
        help = "Prefix appended to the URI (ie. '/pastebin')",
        default_value = ""
    )]
    uri_prefix: String,

    #[structopt(
        long = "slug-charset",
        help = "Character set (expressed as rust compatible regex) to use for generating the URL slug",
        default_value = "[A-Za-z0-9_-]"
    )]
    slug_charset: String,

    #[structopt(long = "slug-len", help = "Length of URL slug", default_value = "21")]
    slug_len: usize,

    #[structopt(
        long = "ui-expiry-times",
        help = "List of paste expiry times redered in the UI dropdown selector",
        default_value = "5 minutes, 10 minutes, 1 hour, 1 day, 1 week, 1 month, 1 year, Never"
    )]
    ui_expiry_times: Vec<String>,

    #[structopt(long = "ui-line-numbers", help = "Display line numbers")]
    ui_line_numbers: bool,

    #[structopt(
        long = "plugins",
        help = "Enable additional functionalities (ie. prism, mermaid)",
        default_value = "prism"
    )]
    plugins: Vec<String>,
}

/// Carries the effective public host+scheme derived from reverse-proxy headers.
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
                scheme: scheme.unwrap_or_else(|| String::from("http")),
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

    let port = if vec![443u16, 80].contains(&cfg.port) {
        String::new()
    } else {
        format!(":{}", cfg.port)
    };
    let scheme = if cfg.tls_certs.is_some() { "https" } else { "http" };
    format!("{}://{}{}", scheme, cfg.address, port)
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

    let content = handlebars.render_template(html.as_str(), &map).unwrap();

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
    let url = format!("{url}/{id}", url = get_url(cfg, req_host), id = id);

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
        lang.unwrap_or_else(|| String::from("markup")),
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

#[get("/<id>?<lang>")]
async fn get<'r>(
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
    let html = String::from_utf8_lossy(resources.get("/static/index.html").unwrap()).to_string();

    // handle missing entry
    let root = match get_entry_data(&id, state) {
        Ok(x) => x,
        Err(e) => {
            let err_kind = match e.kind() {
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

            let content = handlebars.render_template(html.as_str(), &map).unwrap();

            return CustomResponse(
                Response::build()
                    .status(err_kind)
                    .header(ContentType::HTML)
                    .sized_body(content.len(), Cursor::new(content))
                    .finalize(),
            );
        }
    };

    // handle existing entry
    let entry = root_as_entry(&root).unwrap();
    let selected_lang = lang
        .unwrap_or_else(|| entry.lang().unwrap().to_string())
        .to_lowercase();

    let mut pastebin_cls = Vec::new();
    if cfg.ui_line_numbers {
        pastebin_cls.push("line-numbers".to_string());
    }

    pastebin_cls.push(format!("language-{}", selected_lang));

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
        map["msg"] = json!(format!("This paste will expire on {}.", time));
        map["level"] = json!("info");
        map["glyph"] = json!("far fa-clock");
    }

    if entry.encrypted() {
        map["is_encrypted"] = json!("true");
    }

    let content = handlebars.render_template(html.as_str(), &map).unwrap();

    CustomResponse(
        Response::build()
            .status(Status::Ok)
            .header(ContentType::HTML)
            .sized_body(content.len(), Cursor::new(content))
            .finalize(),
    )
}

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
    let html = String::from_utf8_lossy(resources.get("/static/index.html").unwrap()).to_string();
    let msg = msg.unwrap_or_else(|| String::from(""));
    let level = level.unwrap_or_else(|| String::from("secondary"));
    let glyph = glyph.unwrap_or_else(|| String::from(""));
    let url = url.unwrap_or_else(|| String::from(""));
    let root: Vec<u8>;

    let mut map = json!({
        "is_editable": "true",
        "version": VERSION,
        "msg": msg,
        "level": level,
        "glyph": glyph,
        "url": url,
        "uri_prefix": cfg.uri_prefix,
        "ui_expiry_times": ui_expiry_times.inner(),
        "ui_expiry_default": ui_expiry_default.inner(),
        "js_imports": plugin_manager.js_imports(),
        "css_imports": plugin_manager.css_imports(),
        "js_init": plugin_manager.js_init(),
    });

    if let Some(id) = id {
        root = get_entry_data(&id, state).unwrap();
        let entry = root_as_entry(&root).unwrap();

        if entry.encrypted() {
            map["is_encrypted"] = json!("true");
        }

        map["pastebin_code"] = json!(std::str::from_utf8(entry.data().unwrap().bytes()).unwrap());
    }

    let content = handlebars.render_template(html.as_str(), &map).unwrap();

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
    // handle missing entry
    let root = match get_entry_data(&id, state) {
        Ok(x) => x,
        Err(e) => {
            let err_kind = match e.kind() {
                io::ErrorKind::NotFound => Status::NotFound,
                _ => Status::InternalServerError,
            };

            return CustomResponse(Response::build().status(err_kind).finalize());
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
    let pth = format!("/static/{}", resource);
    let ext = get_extension(resource.as_str()).replace(".", "");

    let content = match resources.get(pth.as_str()) {
        Some(data) => *data,
        None => {
            let html =
                String::from_utf8_lossy(resources.get("/static/index.html").unwrap()).to_string();

            return get_error_response(
                handlebars.inner(),
                cfg.uri_prefix.clone(),
                html,
                Status::NotFound,
            );
        }
    };
    let content_type = ContentType::from_extension(ext.as_str()).unwrap();

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
    let url = String::from(
        Path::new(cfg.uri_prefix.as_str())
            .join("new")
            .to_str()
            .unwrap(),
    );

    rocket::response::Redirect::to(url)
}

fn rocket_instance(pastebin_config: PastebinConfig) -> rocket::Rocket<rocket::Build> {
    let workers = if pastebin_config.workers != 0 {
        pastebin_config.workers
    } else {
        num_cpus::get() * 2
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

    // handle tls cert setup
    if pastebin_config.tls_certs.is_some() && pastebin_config.tls_key.is_some() {
        rocket_config.tls = Some(rocket::config::TlsConfig::from_paths(
            pastebin_config.tls_certs.clone().unwrap(),
            pastebin_config.tls_key.clone().unwrap(),
        ));
    }

    // setup db
    let db = DB::open_default(pastebin_config.db_path.clone()).unwrap();
    let mut db_opts = Options::default();

    db_opts.create_if_missing(true);
    db_opts.set_compaction_filter("ttl_entries", compaction_filter_expired_entries);

    // define slug URL alphabet
    let alphabet = {
        let re = Regex::new(&pastebin_config.slug_charset).unwrap();

        let mut tmp = [0; 4];
        let mut alphabet: Vec<char> = vec![];

        // match all printable ASCII characters
        for i in 0x20..0x7e as u8 {
            let c = i as char;

            if re.is_match(c.encode_utf8(&mut tmp)) {
                alphabet.push(c);
            }
        }

        alphabet
    };

    // setup drop down expiry menu (for instance 1m, 20m, 1 year, never)
    let ui_expiry_times = {
        let mut all = vec![];
        for item in pastebin_config.ui_expiry_times.clone() {
            for sub_elem in item.split(',') {
                if sub_elem.trim().to_lowercase() == "never" {
                    all.push((sub_elem.trim().to_string(), 0));
                } else {
                    all.push((
                        sub_elem.trim().to_string(),
                        parse_duration(sub_elem).unwrap().as_secs(),
                    ));
                }
            }
        }

        all
    };

    let ui_expiry_default: String = ui_expiry_times
        .iter()
        .filter_map(|(name, val)| {
            if *val == pastebin_config.ttl {
                Some(name.clone())
            } else {
                None
            }
        })
        .collect();

    if ui_expiry_default.is_empty() {
        panic!("the TTL flag should match one of the ui-expiry-times option");
    }

    if pastebin_config.slug_len == 0 {
        panic!("slug_len must be larger than zero");
    }

    if alphabet.is_empty() {
        panic!("selected slug alphabet is empty, please check if slug_charset is a valid regex");
    }

    let plugins: Vec<Box<dyn Plugin>> = pastebin_config
        .plugins
        .iter()
        .map(|t| match t.as_str() {
            "prism" => Box::new(plugins::prism::new()),
            "mermaid" => Box::new(plugins::mermaid::new()),
            _ => panic!("unknown plugin provided"),
        })
        .map(|x| x as Box<dyn plugins::plugin::Plugin>)
        .collect();

    let plugin_manager = plugins::new(plugins);
    let uri_prefix = pastebin_config.uri_prefix.clone();

    // run rocket
    rocket::custom(rocket_config)
        .manage(pastebin_config)
        .manage(db)
        .manage(formatter::new())
        .manage(plugin_manager)
        .manage(alphabet)
        .manage(ui_expiry_times)
        .manage(ui_expiry_default)
        .mount(
            if uri_prefix.is_empty() {
                "/"
            } else {
                uri_prefix.as_str()
            },
            routes![index, create, remove, get, get_new, get_raw, get_binary, get_static],
        )
}

#[rocket::main]
async fn main() {
    let pastebin_config = PastebinConfig::from_args();
    rocket_instance(pastebin_config)
        .launch()
        .await
        .expect("rocket failed to launch");
}
