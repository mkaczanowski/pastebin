use std::collections::HashMap;

pub trait Plugin<'r>: Sync + Send {
    fn css_imports(&self) -> &Vec<&'r str>;
    fn js_imports(&self) -> &Vec<&'r str>;
    fn js_init(&self) -> Option<&'r str>;
    fn static_resources(&self) -> &HashMap<&'r str, &'r [u8]>;
}

#[derive(Debug)]
pub struct PastebinPlugin<'r> {
    pub css_imports: Vec<&'r str>,
    pub js_imports: Vec<&'r str>,
    pub js_init: Option<&'r str>,
    pub static_resources: HashMap<&'r str, &'r [u8]>,
}

impl<'r> Plugin<'r> for PastebinPlugin<'r> {
    fn css_imports(&self) -> &Vec<&'r str> {
        &self.css_imports
    }

    fn js_imports(&self) -> &Vec<&'r str> {
        &self.js_imports
    }

    fn js_init(&self) -> Option<&'r str> {
        self.js_init
    }

    fn static_resources(&self) -> &HashMap<&'r str, &'r [u8]> {
        &self.static_resources
    }
}

pub struct PluginManagerBuilder<'r> {
    manager: PluginManager<'r>,
}

impl<'r> PluginManagerBuilder<'r> {
    pub fn plugins(&mut self, plugins: Vec<Box<dyn Plugin<'r>>>) -> &mut PluginManagerBuilder<'r> {
        self.manager.set_plugins(plugins);
        self
    }

    pub fn css_imports(&mut self, css_imports: Vec<&'r str>) -> &mut PluginManagerBuilder<'r> {
        self.manager.set_css_imports(css_imports);
        self
    }

    pub fn js_imports(&mut self, js_imports: Vec<&'r str>) -> &mut PluginManagerBuilder<'r> {
        self.manager.set_js_imports(js_imports);
        self
    }

    pub fn static_resources(
        &mut self,
        static_resources: HashMap<&'r str, &'r [u8]>,
    ) -> &mut PluginManagerBuilder<'r> {
        self.manager.set_static_resources(static_resources);
        self
    }

    pub fn finalize(&mut self) -> PluginManager<'r> {
        self.manager.build_css_imports();
        self.manager.build_js_imports();
        self.manager.build_js_init();
        self.manager.build_static_resources();

        std::mem::replace(&mut self.manager, PluginManager::new())
    }
}

pub struct PluginManager<'r> {
    // plugins are used to build up the static members of the struct, for instance:
    //   * js_imports (ie. "{{uri_prefix}}/static/prism.js")
    //   * static_resources (files under static/ directory - compiled with the binary)
    plugins: Vec<Box<dyn Plugin<'r>>>,

    css_imports: Vec<&'r str>,
    js_imports: Vec<&'r str>,
    js_init: Vec<&'r str>,
    static_resources: HashMap<&'r str, &'r [u8]>,
}

impl<'r> PluginManager<'r> {
    pub fn new() -> PluginManager<'r> {
        PluginManager {
            plugins: vec![],
            css_imports: vec![],
            js_imports: vec![],
            js_init: vec![],
            static_resources: HashMap::new(),
        }
    }

    pub fn build() -> PluginManagerBuilder<'r> {
        PluginManagerBuilder {
            manager: PluginManager::new(),
        }
    }

    pub fn set_plugins(&mut self, plugins: Vec<Box<dyn Plugin<'r>>>) {
        self.plugins = plugins;
    }

    pub fn set_css_imports(&mut self, css_imports: Vec<&'r str>) {
        self.css_imports = css_imports;
    }

    pub fn css_imports(&self) -> Vec<&'r str> {
        self.css_imports.clone()
    }

    pub fn set_js_imports(&mut self, js_imports: Vec<&'r str>) {
        self.js_imports = js_imports;
    }

    pub fn js_imports(&self) -> Vec<&'r str> {
        self.js_imports.clone()
    }

    pub fn set_js_init(&mut self, js_init: Vec<&'r str>) {
        self.js_init = js_init;
    }

    pub fn js_init(&self) -> Vec<&'r str> {
        self.js_init.clone()
    }

    pub fn set_static_resources(&mut self, static_resources: HashMap<&'r str, &'r [u8]>) {
        self.static_resources = static_resources;
    }

    pub fn static_resources(&self) -> HashMap<&'r str, &'r [u8]> {
        self.static_resources.clone()
    }

    fn build_css_imports(&mut self) {
        self.set_css_imports(
            self.plugins
                .iter()
                .flat_map(|p| p.css_imports().into_iter())
                .chain((&self.css_imports).into_iter())
                .map(|&val| val)
                .collect(),
        )
    }

    fn build_js_imports(&mut self) {
        self.set_js_imports(
            self.plugins
                .iter()
                .flat_map(|p| p.js_imports().into_iter())
                .chain((&self.js_imports).into_iter())
                .map(|&val| val)
                .collect(),
        )
    }

    fn build_js_init(&mut self) {
        self.set_js_init(
            self.plugins
                .iter()
                .flat_map(|p| p.js_init().into_iter())
                .chain((&self.js_init).into_iter().map(|&val| val))
                .collect(),
        )
    }

    fn build_static_resources(&mut self) {
        self.set_static_resources(
            self.plugins
                .iter()
                .flat_map(|p| p.static_resources().into_iter())
                .chain((&self.static_resources).into_iter())
                .map(|(&key, &val)| (key, val))
                .collect(),
        )
    }
}
