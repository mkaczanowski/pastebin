#![feature(proc_macro_hygiene, decl_macro)]
#![allow(clippy::too_many_arguments)]

#[macro_use]
extern crate rocket;
#[macro_use]
extern crate structopt_derive;
#[macro_use]
extern crate maplit;
extern crate flatbuffers;
extern crate handlebars;
extern crate nanoid;
extern crate num_cpus;
extern crate speculate;
extern crate structopt;
extern crate chrono;

#[macro_use]
mod lib;
use lib::{compaction_filter_expired_entries, get_entry_data, get_extension, new_entry};

mod api_generated;
use api_generated::api::get_root_as_entry;

use std::collections::HashMap;
use std::io;
use std::io::Cursor;

use rocket::config::{Config, Environment};
use rocket::http::{ContentType, Status};
use rocket::response::{Redirect, Response};
use rocket::{Data, State};

use handlebars::Handlebars;
use nanoid::nanoid;
use rocksdb::{Options, DB};
use structopt::StructOpt;
use chrono::NaiveDateTime;
use speculate::speculate;

speculate! {
    use super::rocket;
    use rocket::local::Client;
    use rocket::http::Status;

    before {
        use tempdir::TempDir;

        // setup temporary database
        let tmp_dir = TempDir::new("rocks_db_test").unwrap();
        let file_path = tmp_dir.path().join("database");
        let mut pastebin_config = PastebinConfig::from_args();
        pastebin_config.db_path = file_path.to_str().unwrap().to_string();
        let rocket = rocket(pastebin_config);

        // init rocket client
        let client = Client::new(rocket).expect("invalid rocket instance");
    }

    #[allow(dead_code)]
    fn insert_data<'r>(client: &'r Client, data: &str, path: &str) -> String {
        let mut response = client.post(path)
            .body(data)
            .dispatch();
        assert_eq!(response.status(), Status::Ok);

        // retrieve paste ID
        let url = response.body_string().unwrap();
        let id = url.split('/').collect::<Vec<&str>>().last().cloned().unwrap();

        id.to_string()
    }

    #[allow(dead_code)]
    fn get_data(client: &Client, path: String) -> rocket::local::LocalResponse {
        client.get(format!("/{}", path)).dispatch()
    }

    it "can get create and fetch paste" {
        // store data via post request
        let id = insert_data(&client, "random_test_data_to_be_checked", "/");

        // retrieve the data via get request
        let mut response = get_data(&client, id);
        assert_eq!(response.status(), Status::Ok);
        assert_eq!(response.content_type(), Some(ContentType::HTML));
        assert!(response.body_string().unwrap().contains("random_test_data_to_be_checked"));
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
        let mut response = get_data(&client, format!("raw/{}", id));
        assert_eq!(response.status(), Status::Ok);
        assert_eq!(response.content_type(), Some(ContentType::Plain));
        assert!(response.body_string().unwrap().contains("random_test_data_to_be_checked"));
    }

    it "can download contents" {
        // store data via post request
        let id = insert_data(&client, "random_test_data_to_be_checked", "/");

        // retrieve the data via get request
        let mut response = get_data(&client, format!("download/{}", id));
        assert_eq!(response.status(), Status::Ok);
        assert_eq!(response.content_type(), Some(ContentType::Binary));
        assert!(response.body_string().unwrap().contains("random_test_data_to_be_checked"));
    }

    it "can clone contents" {
        // store data via post request
        let id = insert_data(&client, "random_test_data_to_be_checked", "/");

        // retrieve the data via get request
        let mut response = get_data(&client, format!("new?id={}", id));
        assert_eq!(response.status(), Status::Ok);
        assert_eq!(response.content_type(), Some(ContentType::HTML));
        assert!(response.body_string().unwrap().contains("random_test_data_to_be_checked"));
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
        let mut response = client.get("/static/favicon.ico").dispatch();
        let contents = std::fs::read("static/favicon.ico").unwrap();

        assert_eq!(response.status(), Status::Ok);
        assert_eq!(response.body_bytes(), Some(contents));
    }
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

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
        long = "environment",
        help = "Rocket server environment",
        default_value = "production"
    )]
    environment: String,

    #[structopt(
        long = "workers",
        help = "Number of concurrent thread workers",
        default_value = "8"
    )]
    workers: u16,

    #[structopt(
        long = "keep-alive",
        help = "Keep-alive timeout in seconds",
        default_value = "5"
    )]
    keep_alive: u32,

    #[structopt(long = "log", help = "Max log level", default_value = "normal")]
    log: rocket::config::LoggingLevel,

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
}

fn get_url(cfg: &PastebinConfig) -> String {
    let port = if vec![443, 80].contains(&cfg.port) {
        String::from("")
    } else {
        format!(":{}", cfg.port)
    };
    let scheme = if cfg.tls_certs.is_some() {
        "https"
    } else {
        "http"
    };

    format!(
        "{scheme}://{address}{port}",
        scheme = scheme,
        port = port,
        address = cfg.address,
    )
}

fn get_error_response(html: String, status: Status, cfg: &PastebinConfig) -> Response {
    let map = btreemap! {
        "hostname" => cfg.address.as_str(),
        "version" => VERSION,
        "is_error" => "true",
    };

    let content = Handlebars::new()
        .render_template(html.as_str(), &map)
        .unwrap();

    Response::build()
        .status(status)
        .sized_body(Cursor::new(content))
        .finalize()
}

#[post("/?<lang>&<ttl>&<burn>&<encrypted>", data = "<paste>")]
fn create(
    paste: Data,
    state: State<DB>,
    cfg: State<PastebinConfig>,
    lang: Option<String>,
    ttl: Option<u64>,
    burn: Option<bool>,
    encrypted: Option<bool>,
) -> Result<String, io::Error> {
    let id = nanoid!();
    let url = format!("{url}/{id}", url = get_url(cfg.inner()), id = id);

    let mut writer: Vec<u8> = vec![];
    new_entry(
        &mut writer,
        &mut paste.open(),
        lang.unwrap_or_else(|| String::from("markup")),
        ttl.unwrap_or(cfg.ttl),
        burn.unwrap_or(false),
        encrypted.unwrap_or(false),
    );

    state.put(id, writer).unwrap();

    Ok(url)
}

#[delete("/<id>")]
fn remove(id: String, state: State<DB>) -> Result<(), rocksdb::Error> {
    state.delete(id)
}

#[get("/<id>?<lang>")]
fn get<'r>(
    id: String,
    lang: Option<String>,
    state: State<'r, DB>,
    resources: State<'r, HashMap<&str, &[u8]>>,
    cfg: State<PastebinConfig>,
) -> Response<'r> {
    let html = String::from_utf8_lossy(resources.get("../static/index.html").unwrap()).to_string();

    // handle missing entry
    let root = match get_entry_data(&id, &state) {
        Ok(x) => x,
        Err(e) => {
            let err_kind = match e.kind() {
                io::ErrorKind::NotFound => Status::NotFound,
                _ => Status::InternalServerError,
            };

            let map = btreemap! {
                "hostname" => cfg.address.as_str(),
                "version" => VERSION,
                "is_error" => "true",
            };

            let content = Handlebars::new()
                .render_template(html.as_str(), &map)
                .unwrap();

            return Response::build()
                .status(err_kind)
                .sized_body(Cursor::new(content))
                .finalize();
        }
    };

    // handle existing entry
    let entry = get_root_as_entry(&root);
    let selected_lang = lang
        .unwrap_or_else(|| entry.lang().unwrap().to_string())
        .to_lowercase();

    let mut map = btreemap! {
        "is_created" => "true".to_string(),
        "pastebin_code" => std::str::from_utf8(entry.data().unwrap()).unwrap().to_string(),
        "pastebin_id" => id,
        "pastebin_language" => selected_lang,
        "hostname" => cfg.address.clone(),
        "version" => VERSION.to_string(),
    };

    if entry.burn() {
        map.insert(
            "msg",
            "FOR YOUR EYES ONLY. The paste is gone, after you close this window.".to_string(),
        );
        map.insert("level", "warning".to_string());
        map.insert("is_burned", "true".to_string());
        map.insert("glyph", "fa fa-fire".to_string());
    } else if entry.expiry_timestamp() != 0 {
        let time = NaiveDateTime::from_timestamp(entry.expiry_timestamp() as i64, 0).format("%Y-%m-%d %H:%M:%S");
        map.insert("msg", format!("This paste will expire on {}.", time));
        map.insert("level", "info".to_string());
        map.insert("glyph", "far fa-clock".to_string());
    }

    if entry.encrypted() {
        map.insert("is_encrypted", "true".to_string());
    }

    let content = Handlebars::new()
        .render_template(html.as_str(), &map)
        .unwrap();

    Response::build()
        .status(Status::Ok)
        .header(ContentType::HTML)
        .sized_body(Cursor::new(content))
        .finalize()
}

#[get("/new?<id>&<level>&<msg>&<glyph>&<url>")]
fn get_new<'r>(
    state: State<'r, DB>,
    resources: State<'r, HashMap<&str, &[u8]>>,
    cfg: State<PastebinConfig>,
    id: Option<String>,
    level: Option<String>,
    glyph: Option<String>,
    msg: Option<String>,
    url: Option<String>,
) -> Response<'r> {
    let html = String::from_utf8_lossy(resources.get("../static/index.html").unwrap()).to_string();
    let msg = msg.unwrap_or_else(|| String::from(""));
    let level = level.unwrap_or_else(|| String::from("secondary"));
    let glyph = glyph.unwrap_or_else(|| String::from(""));
    let url = url.unwrap_or_else(|| String::from(""));
    let root: Vec<u8>;

    let mut map = btreemap! {
        "is_editable" => "true",
        "hostname" => cfg.address.as_str(),
        "version" => VERSION,
        "msg" => msg.as_str(),
        "level" => level.as_str(),
        "glyph" => glyph.as_str(),
        "url" => url.as_str(),
    };

    if let Some(id) = id {
        root = get_entry_data(&id, &state).unwrap();
        let entry = get_root_as_entry(&root);

        if entry.encrypted() {
            map.insert("is_encrypted", "true");
        }

        map.insert(
            "pastebin_code",
            std::str::from_utf8(entry.data().unwrap()).unwrap(),
        );
    }

    let content = Handlebars::new()
        .render_template(html.as_str(), &map)
        .unwrap();

    Response::build()
        .status(Status::Ok)
        .header(ContentType::HTML)
        .sized_body(Cursor::new(content))
        .finalize()
}

#[get("/raw/<id>")]
fn get_raw(id: String, state: State<DB>) -> Response {
    // handle missing entry
    let root = match get_entry_data(&id, &state) {
        Ok(x) => x,
        Err(e) => {
            let err_kind = match e.kind() {
                io::ErrorKind::NotFound => Status::NotFound,
                _ => Status::InternalServerError,
            };

            return Response::build().status(err_kind).finalize();
        }
    };

    let entry = get_root_as_entry(&root);
    let mut data: Vec<u8> = vec![];

    io::copy(&mut entry.data().unwrap(), &mut data).unwrap();

    Response::build()
        .status(Status::Ok)
        .header(ContentType::Plain)
        .sized_body(Cursor::new(data))
        .finalize()
}

#[get("/download/<id>")]
fn get_binary(id: String, state: State<DB>) -> Response {
    let response = get_raw(id, state);
    Response::build_from(response)
        .header(ContentType::Binary)
        .finalize()
}

#[get("/static/<resource>")]
fn get_static<'r>(
    resource: String,
    resources: State<'r, HashMap<&str, &[u8]>>,
    cfg: State<'r, PastebinConfig>,
) -> Response<'r> {
    let pth = format!("../static/{}", resource);
    let ext = get_extension(resource.as_str()).replace(".", "");

    let content = match resources.get(pth.as_str()) {
        Some(data) => data,
        None => {
            let html =
                String::from_utf8_lossy(resources.get("../static/index.html").unwrap()).to_string();
            return get_error_response(html, Status::NotFound, cfg.inner());
        }
    };
    let content_type = ContentType::from_extension(ext.as_str()).unwrap();

    Response::build()
        .status(Status::Ok)
        .header(content_type)
        .sized_body(Cursor::new(content.iter()))
        .finalize()
}

#[get("/")]
fn index() -> Redirect {
    Redirect::to("/new")
}

fn rocket(pastebin_config: PastebinConfig) -> rocket::Rocket {
    // parse command line opts
    let environ: Environment = pastebin_config.environment.parse().unwrap();
    let mut rocket_config = Config::build(environ)
        .address(pastebin_config.address.clone())
        .port(pastebin_config.port)
        .workers(pastebin_config.workers)
        .keep_alive(pastebin_config.keep_alive)
        .log_level(pastebin_config.log)
        .finalize()
        .unwrap();

    // handle tls cert setup
    if pastebin_config.tls_certs.is_some() && pastebin_config.tls_key.is_some() {
        rocket_config
            .set_tls(
                pastebin_config.tls_certs.clone().unwrap().as_str(),
                pastebin_config.tls_key.clone().unwrap().as_str(),
            )
            .unwrap();
    }

    // setup db
    let db = DB::open_default(pastebin_config.db_path.clone()).unwrap();
    let mut db_opts = Options::default();

    db_opts.create_if_missing(true);
    db_opts.set_compaction_filter("ttl_entries", compaction_filter_expired_entries);

    let resources = load_static_resources!(
        "../static/index.html",
        "../static/custom.js",
        "../static/custom.css",
        "../static/prism.js",
        "../static/prism.css",
        "../static/favicon.ico"
    );

    // run rocket
    rocket::custom(rocket_config)
        .manage(pastebin_config)
        .manage(db)
        .manage(resources)
        .mount(
            "/",
            routes![index, create, remove, get, get_new, get_raw, get_binary, get_static],
        )
}

fn main() {
    let pastebin_config = PastebinConfig::from_args();
    rocket(pastebin_config).launch();
}
