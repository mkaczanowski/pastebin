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

    let rendered = match uri.starts_with("/") {
        true => format!("{}{}", prefix, uri),
        false => uri,
    };

    out.write(rendered.as_ref())?;
    Ok(())
}
