use std::collections::HashMap;

use crate::plugins::plugin::PastebinPlugin;

pub fn new() -> PastebinPlugin {
    PastebinPlugin {
        css_imports: vec![],
        js_imports: vec![
            "https://cdnjs.cloudflare.com/ajax/libs/mermaid/11.12.0/mermaid.min.js",
        ],
        js_init: Some(
            "mermaid.initialize({startOnLoad: false}); \
             mermaid.run({querySelector: '.language-mermaid'});",
        ),
        static_resources: HashMap::new(),
    }
}
