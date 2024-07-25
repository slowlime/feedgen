mod api;
mod types;

use anyhow::{anyhow, Context, Result};
use mlua::{ChunkMode, Function, Lua, LuaOptions, RegistryKey, StdLib};
use tracing::{debug, warn};

use crate::config;

use self::api::add_feedgen_api;
use self::types::{Buffer, LuaEntry};

use super::{Entry, Extractor};

fn make_vm() -> Result<Lua> {
    let lua_libs = StdLib::COROUTINE | StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH;
    let lua = Lua::new_with(lua_libs, LuaOptions::new().catch_rust_panics(false))?;
    add_feedgen_api(&lua).context("could not register the Feedgen API")?;

    Ok(lua)
}

pub struct LuaExtractor {
    lua: Lua,
    extract_key: RegistryKey,
}

impl LuaExtractor {
    pub fn from_cfg(cfg: &config::LuaExtractorConfig) -> Result<Self> {
        debug!("Loading a Lua extractor script: {}", cfg.path.display());

        let lua = make_vm().context("could not set up a Lua VM")?;
        lua.load(cfg.path.as_path())
            .set_mode(ChunkMode::Text)
            .exec()
            .with_context(|| anyhow!("could not run the Lua script at `{}`", cfg.path.display()))?;
        let extract: Function<'_> = lua
            .globals()
            .get("extract")
            .context("found no suitable `extract` function")?;
        let extract_key = lua
            .create_registry_value(extract)
            .context("could not save the `extract` function in the Lua registry")?;

        Ok(Self { lua, extract_key })
    }
}

impl Extractor for LuaExtractor {
    fn extract(&mut self, ctx: super::Context<'_>, html: &str) -> Result<Vec<Entry>> {
        let buf = Buffer::from(html);
        let extract: Function<'_> = self
            .lua
            .registry_value(&self.extract_key)
            .context("could not retrieve the `extract` function")?;
        let entries: Vec<LuaEntry> = extract
            .call(buf)
            .context("running the `extract` function failed")?;

        Ok(entries
            .into_iter()
            .enumerate()
            .filter_map(|(idx, entry)| {
                let url = ctx
                    .fetch_url()
                    .join(&entry.url)
                    .inspect_err(|e| {
                        warn!("The URL for entry #{} could not be parsed: {e:#}", idx + 1);
                    })
                    .ok()?;

                Some(Entry {
                    id: entry.id,
                    title: entry.title,
                    description: entry.description,
                    url,
                    author: entry.author,
                    pub_date: entry.pub_date,
                })
            })
            .collect())
    }
}
