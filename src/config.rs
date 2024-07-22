mod types;

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use reqwest::Url;
use serde::Deserialize;
use tracing::{debug, info};

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

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ExtractorConfig {
    #[serde(rename = "xpath")]
    XPath(XPathExtractorConfig),
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

        let cfg = toml::from_str(&contents)
            .with_context(|| anyhow!("could not load the config file `{}`", path.display()))?;

        info!("Loaded a config file `{}`", path.display());

        return Ok(cfg);
    }

    info!("Using the default config");

    Ok(Default::default())
}
