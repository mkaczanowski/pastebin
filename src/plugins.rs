pub mod mermaid;
pub mod plugin;
pub mod prism;

use std::collections::HashMap;

pub fn new<'r>(plugins: Vec<Box<dyn plugin::Plugin<'r>>>) -> plugin::PluginManager<'r> {
    let base_static_resources = load_static_resources!(
    "/static/index.html" => "../static/index.html",
    "/static/custom.js" => "../static/custom.js",
    "/static/custom.css" => "../static/custom.css",
    "/static/favicon.ico" => "../static/favicon.ico"
    );

    let base_css_imports = vec![
        "https://cdnjs.cloudflare.com/ajax/libs/twitter-bootstrap/4.0.0/css/bootstrap.min.css",
        "https://cdnjs.cloudflare.com/ajax/libs/font-awesome/5.13.0/css/all.min.css",
        "/static/custom.css",
    ];

    let base_js_imports = vec![
        "https://cdnjs.cloudflare.com/ajax/libs/jquery/3.2.1/jquery.min.js",
        "https://cdnjs.cloudflare.com/ajax/libs/crypto-js/4.0.0/crypto-js.min.js",
        "https://cdnjs.cloudflare.com/ajax/libs/popper.js/1.12.9/umd/popper.min.js",
        "https://cdnjs.cloudflare.com/ajax/libs/twitter-bootstrap/4.0.0/js/bootstrap.min.js",
        "https://cdnjs.cloudflare.com/ajax/libs/clipboard.js/2.0.4/clipboard.min.js",
        "https://cdnjs.cloudflare.com/ajax/libs/bootstrap-notify/0.2.0/js/bootstrap-notify.min.js",
        "/static/custom.js",
    ];

    plugin::PluginManager::build()
        .plugins(plugins)
        .static_resources(base_static_resources)
        .css_imports(base_css_imports)
        .js_imports(base_js_imports)
        .finalize()
}
