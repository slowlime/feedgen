mod types;

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use reqwest::Url;
use serde::Deserialize;
use tracing::{debug, info};
use take_mut::take;

use crate::xpath::XPath;

pub use self::types::*;

fn default_fetch_interval() -> Duration {
    Config::default().fetch_interval
}

fn default_max_initial_fetch_sleep() -> Duration {
    Config::default().max_initial_fetch_sleep
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Config {
    pub bind_addr: String,
    pub db_path: PathBuf,
    pub cache_dir: Option<PathBuf>,
    pub feeds: HashMap<String, Feed>,

    #[serde(default = "default_fetch_interval")]
    pub fetch_interval: Duration,

    #[serde(default = "default_max_initial_fetch_sleep")]
    pub max_initial_fetch_sleep: Duration,
}

impl Config {
    pub fn update(&mut self, args: crate::cli::Args) {
        fn set_if_some<T>(dst: &mut T, v: Option<T>) {
            if let Some(v) = v {
                *dst = v;
            }
        }

        set_if_some(&mut self.bind_addr, args.bind_addr);
        set_if_some(&mut self.db_path, args.db_path);
        set_if_some(&mut self.cache_dir, args.cache_dir.map(Some));
    }

    pub fn resolve_relative_paths(&mut self, config_dir: impl AsRef<Path>) {
        let config_dir = config_dir.as_ref();

        // do the dance for safety (so that I don't forget to update this after adding new fields).
        take(self, |mut this| {
            for feed in this.feeds.values_mut() {
                feed.resolve_relative_paths(config_dir);
            }

            Self {
                bind_addr: this.bind_addr,
                db_path: config_dir.join(&this.db_path),
                cache_dir: this.cache_dir.map(|cache_dir| config_dir.join(cache_dir)),
                feeds: this.feeds,
                fetch_interval: this.fetch_interval,
                max_initial_fetch_sleep: this.max_initial_fetch_sleep,
            }
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            bind_addr: "127.0.0.1:20654".into(),
            db_path: "./feedgen.sqlite3".into(),
            cache_dir: None,
            fetch_interval: Duration::from_secs(7200),
            max_initial_fetch_sleep: Duration::from_secs(45),
            feeds: Default::default(),
        }
    }
}

fn default_feed_enabled() -> bool {
    true
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Feed {
    #[serde(default = "default_feed_enabled")]
    pub enabled: bool,

    pub request_url: Url,
    pub extractor: ExtractorConfig,
    pub fetch_interval: Option<Duration>,
}

impl Feed {
    pub fn resolve_relative_paths(&mut self, config_dir: impl AsRef<Path>) {
        let config_dir = config_dir.as_ref();

        take(self, |mut this| {
            this.extractor.resolve_relative_paths(config_dir);

            Self {
                enabled: this.enabled,
                request_url: this.request_url,
                extractor: this.extractor,
                fetch_interval: this.fetch_interval,
            }
        })
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ExtractorConfig {
    #[serde(rename = "xpath")]
    XPath(XPathExtractorConfig),

    Lua(LuaExtractorConfig),
}

impl ExtractorConfig {
    pub fn resolve_relative_paths(&mut self, config_dir: impl AsRef<Path>) {
        let config_dir = config_dir.as_ref();

        match self {
            Self::XPath(cfg) => cfg.resolve_relative_paths(config_dir),
            Self::Lua(cfg) => cfg.resolve_relative_paths(config_dir),
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct XPathExtractorConfig {
    pub entry: XPath,
    pub id: XPath,
    pub title: XPath,
    pub description: XPath,
    pub url: XPath,
    pub author: Option<XPath>,
    pub pub_date: Option<XPath>,
    pub pub_date_format: Option<DateTimeFormat>,
}

impl XPathExtractorConfig {
    pub fn resolve_relative_paths(&mut self, _config_dir: impl AsRef<Path>) {
        take(self, |this| Self {
            entry: this.entry,
            id: this.id,
            title: this.title,
            description: this.description,
            url: this.url,
            author: this.author,
            pub_date: this.pub_date,
            pub_date_format: this.pub_date_format,
        })
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LuaExtractorConfig {
    pub path: PathBuf,
}

impl LuaExtractorConfig {
    pub fn resolve_relative_paths(&mut self, config_dir: impl AsRef<Path>) {
        let config_dir = config_dir.as_ref();

        take(self, |this| Self {
            path: config_dir.join(this.path),
        })
    }
}

pub fn load(search_paths: &[PathBuf]) -> Result<Config> {
    for path in search_paths {
        debug!("Trying to load {}", path.display());
        let mut contents = String::new();

        {
            let mut f = match File::open(path) {
                Ok(f) => f,

                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    debug!(file = %path.display(), "File not found, skipping");
                    continue;
                }

                Err(e) => {
                    return Err(e)
                        .context(anyhow!("could not load a config file `{}`", path.display()));
                }
            };

            f.read_to_string(&mut contents).with_context(|| {
                anyhow!(
                    "could not read the contents of a config file `{}`",
                    path.display()
                )
            })?;
        }

        let mut cfg: Config = toml::from_str(&contents)
            .with_context(|| anyhow!("could not load the config file `{}`", path.display()))?;

        if let Some(parent) = path.parent() {
            cfg.resolve_relative_paths(parent);
        }

        info!("Loaded a config file `{}`", path.display());

        return Ok(cfg);
    }

    info!("Using the default config");

    Ok(Default::default())
}
