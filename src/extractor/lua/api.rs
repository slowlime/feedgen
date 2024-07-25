use std::cell::Cell;

use anyhow::{anyhow, Context, Result};
use mlua::Result as LuaResult;
use mlua::{FromLuaMulti, IntoLuaMulti, Lua, MultiValue, Table as LuaTable};
use scraper::Html;
use tracing::{debug, error, info, trace, warn};

use super::types::{Buffer, LuaHtml, SelectorWrapper};

fn parse_selector(_lua: &Lua, selector: SelectorWrapper) -> LuaResult<SelectorWrapper> {
    Ok(selector)
}

fn parse_html(_lua: &Lua, buf: Buffer) -> LuaResult<LuaHtml> {
    let html = Html::parse_document(&buf);
    let html = LuaHtml::from(html);

    Ok(html)
}

fn get_caller_info(lua: &Lua) -> String {
    let Some(debug) = lua.inspect_stack(1) else {
        return "<unknown>".into();
    };
    let src = debug.source().short_src.unwrap_or("<unknown>".into());
    let line = debug.curr_line();

    if line > 0 {
        format!("{src}:{line}")
    } else {
        src.into()
    }
}

fn args_to_string(values: MultiValue<'_>, sep: &str) -> String {
    use std::fmt::Write;

    let mut result = String::new();

    for (idx, value) in values.into_iter().enumerate() {
        if idx > 0 {
            result.push_str(sep);
        }

        match value.to_string() {
            Ok(s) => result.push_str(&s),

            Err(e) => {
                let _ = write!(result, "<conversion of arg #{} failed: {e}>", idx + 1);
            }
        }
    }

    result
}

fn log_trace(lua: &Lua, args: MultiValue<'_>) -> LuaResult<()> {
    trace!(
        location = %get_caller_info(lua),
        "{}",
        args_to_string(args, " "),
    );

    Ok(())
}

fn log_debug(lua: &Lua, args: MultiValue<'_>) -> LuaResult<()> {
    debug!(
        location = %get_caller_info(lua),
        "{}",
        args_to_string(args, " "),
    );

    Ok(())
}

fn log_info(lua: &Lua, args: MultiValue<'_>) -> LuaResult<()> {
    info!(
        location = %get_caller_info(lua),
        "{}",
        args_to_string(args, " "),
    );

    Ok(())
}

fn log_warn(lua: &Lua, args: MultiValue<'_>) -> LuaResult<()> {
    warn!(
        location = %get_caller_info(lua),
        "{}",
        args_to_string(args, " "),
    );

    Ok(())
}

fn log_error(lua: &Lua, args: MultiValue<'_>) -> LuaResult<()> {
    error!(
        location = %get_caller_info(lua),
        "{}",
        args_to_string(args, " "),
    );

    Ok(())
}

fn make_warning_emitter() -> impl Fn(&Lua, &str, bool) -> LuaResult<()> + Send + 'static {
    let last_continued = Cell::new(false);

    move |lua, s, cont| {
        let location = get_caller_info(lua);

        if last_continued.get() {
            warn!(%location, "Lua warning (cont.): {s}");
        } else {
            warn!(%location, "Lua warning: {s}");
        }

        last_continued.set(cont);

        Ok(())
    }
}

pub fn add_feedgen_api(lua: &Lua) -> Result<()> {
    let feedgen = lua
        .create_table()
        .context("could not create a table `feedgen`")?;

    fn register<'lua, F, A, R>(
        lua: &'lua Lua,
        tbl: &LuaTable<'lua>,
        name: &str,
        key: &str,
        f: F,
    ) -> Result<()>
    where
        F: Fn(&'lua Lua, A) -> LuaResult<R> + Send + 'static,
        A: FromLuaMulti<'lua>,
        R: IntoLuaMulti<'lua>,
    {
        let f = lua
            .create_function(f)
            .with_context(|| anyhow!("could not create a function `{name}`"))?;
        tbl.set(key, f)
            .with_context(|| anyhow!("could not register `{name}`"))?;

        Ok(())
    }

    macro_rules! register {
        ($($arg:expr),+ $(,)?) => (register(lua, &feedgen, $($arg),+));
    }

    register!("feedgen.parseSelector", "parseSelector", parse_selector)?;
    register!("feedgen.parseHtml", "parseHtml", parse_html)?;

    let log = lua
        .create_table()
        .context("could not create a table `feedgen.log`")?;
    register(lua, &log, "feedgen.log.trace", "trace", log_trace)?;
    register(lua, &log, "feedgen.log.debug", "debug", log_debug)?;
    register(lua, &log, "feedgen.log.info", "info", log_info)?;
    register(lua, &log, "feedgen.log.warn", "warn", log_warn)?;
    register(lua, &log, "feedgen.log.error", "error", log_error)?;

    feedgen
        .set("log", log)
        .context("could not register `feedgen.log`")?;
    lua.globals()
        .set("feedgen", feedgen)
        .context("could not register `feedgen`")?;

    register(lua, &lua.globals(), "print", "print", log_info)?;
    lua.set_warning_function(make_warning_emitter());

    Ok(())
}
