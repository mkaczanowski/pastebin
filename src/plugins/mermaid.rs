use std::collections::HashMap;

use crate::plugins::plugin::PastebinPlugin;

pub fn new() -> PastebinPlugin {
    PastebinPlugin {
        css_imports: vec![],
        js_imports: vec!["https://cdnjs.cloudflare.com/ajax/libs/mermaid/8.8.2/mermaid.min.js"],
        js_init: Some("mermaid.init(undefined, '.language-mermaid');"),
        static_resources: HashMap::new(),
    }
}
