use std::collections::HashMap;

use crate::plugins::plugin::PastebinPlugin;

pub fn new<'r>() -> PastebinPlugin<'r> {
    PastebinPlugin {
        css_imports: vec!["/static/prism.css"],
        js_imports: vec!["/static/prism.js"],
        js_init: Some(
            "var holder = $('#pastebin-code-block:first').get(0); \
            if (holder) { Prism.highlightElement(holder); }",
        ),
        static_resources: load_static_resources! {
            "/static/prism.js" => "../../static/prism.js",
            "/static/prism.css" =>"../../static/prism.css"
        },
    }
}
