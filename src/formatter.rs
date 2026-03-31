use handlebars::{Handlebars, JsonRender};

pub fn new() -> Handlebars<'static> {
    let mut handlebars = Handlebars::new();
    handlebars.register_helper("format_url", Box::new(format_helper));
    handlebars
}

fn format_helper(
    h: &handlebars::Helper,
    _: &Handlebars,
    _: &handlebars::Context,
    _: &mut handlebars::RenderContext,
    out: &mut dyn handlebars::Output,
) -> Result<(), handlebars::RenderError> {
    let prefix_val = h.param(0).ok_or(handlebars::RenderError::from(
        handlebars::RenderErrorReason::ParamNotFoundForIndex("format_url", 0),
    ))?;

    let uri_val = h.param(1).ok_or(handlebars::RenderError::from(
        handlebars::RenderErrorReason::ParamNotFoundForIndex("format_url", 1),
    ))?;

    let prefix = prefix_val.value().render();
    let uri = uri_val.value().render();

    let rendered = if uri.starts_with('/') {
        format!("{prefix}{uri}")
    } else {
        uri
    };

    out.write(&rendered)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render(template: &str, data: serde_json::Value) -> Result<String, handlebars::RenderError> {
        new().render_template(template, &data)
    }

    #[test]
    fn format_url_absolute_uri_prepends_prefix() {
        let result = render(r#"{{format_url prefix uri}}"#, json!({"prefix": "/paste", "uri": "/static/foo.js"})).unwrap();
        assert_eq!(result, "/paste/static/foo.js");
    }

    #[test]
    fn format_url_absolute_uri_with_empty_prefix() {
        let result = render(r#"{{format_url prefix uri}}"#, json!({"prefix": "", "uri": "/static/foo.js"})).unwrap();
        assert_eq!(result, "/static/foo.js");
    }

    #[test]
    fn format_url_relative_uri_passes_through_unchanged() {
        let result = render(r#"{{format_url prefix uri}}"#, json!({"prefix": "/paste", "uri": "https://cdn.example.com/foo.js"})).unwrap();
        assert_eq!(result, "https://cdn.example.com/foo.js");
    }

    #[test]
    fn format_url_missing_first_param_returns_error() {
        assert!(render(r#"{{format_url}}"#, json!({})).is_err());
    }

    #[test]
    fn format_url_missing_second_param_returns_error() {
        assert!(render(r#"{{format_url prefix}}"#, json!({"prefix": "/paste"})).is_err());
    }
}
