use handlebars::{Handlebars, JsonRender, no_escape};

pub fn new<'r>() -> Handlebars<'r> {
    let mut handlebars = Handlebars::new();
    handlebars.register_helper("format_url", Box::new(format_helper));
    handlebars.register_escape_fn(no_escape);

    handlebars
}

fn format_helper(
    h: &handlebars::Helper,
    _: &Handlebars,
    _: &handlebars::Context,
    _: &mut handlebars::RenderContext,
    out: &mut dyn handlebars::Output,
) -> Result<(), handlebars::RenderError> {
    let prefix_val = h.param(0).ok_or(handlebars::RenderError::new(
        "Param 0 is required for format helper.",
    ))?;

    let uri_val = h.param(1).ok_or(handlebars::RenderError::new(
        "Param 1 is required for format helper.",
    ))?;

    let prefix = prefix_val.value().render();
    let uri = uri_val.value().render();

    let rendered = match uri.starts_with("/") {
        true => format!("{}{}", prefix, uri),
        false => uri,
    };

    out.write(rendered.as_ref())?;
    Ok(())
}
