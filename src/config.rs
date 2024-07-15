use std::collections::HashMap;
use std::fmt::{self, Formatter};
use std::fs::File;
use std::io::{self, Read};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::Url;
use serde::de::Visitor;
use serde::{Deserialize, Deserializer};
use tracing::{debug, info};

fn default_fetch_interval() -> Duration {
    Config::default().fetch_interval
}

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub bind_addr: String,
    pub db_path: PathBuf,
    pub cache_dir: Option<PathBuf>,
    pub feeds: HashMap<String, Feed>,

    #[serde(default = "default_fetch_interval")]
    pub fetch_interval: Duration,
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
            fetch_interval: default_fetch_interval(),
            feeds: Default::default(),
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct Feed {
    pub request_url: Url,
    pub extractor: ExtractorConfig,
    pub fetch_interval: Option<Duration>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "kind")]
pub enum ExtractorConfig {
    Xpath(XpathExtractorConfig),
}

#[derive(Deserialize, Debug, Clone)]
pub struct XpathExtractorConfig {
    pub entry: Xpath,
    pub id: Xpath,
    pub title: Xpath,
    pub description: Xpath,
    pub url: Xpath,
    pub author: Option<Xpath>,
}

#[derive(Debug, Clone)]
pub struct Xpath(String);

impl Xpath {
    pub fn compile(&self) -> skyscraper::xpath::Xpath {
        skyscraper::xpath::parse(&self.0).unwrap()
    }
}

impl<'de> Deserialize<'de> for Xpath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct XpathVisitor;

        impl<'de> Visitor<'de> for XpathVisitor {
            type Value = Xpath;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
                write!(formatter, "an XPath expression")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match skyscraper::xpath::parse(v) {
                    Ok(_) => Ok(Xpath(v.to_owned())),
                    Err(e) => Err(E::custom(e)),
                }
            }
        }

        deserializer.deserialize_str(XpathVisitor)
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

        let cfg = toml::from_str(&contents)
            .with_context(|| anyhow!("could not load the config file `{}`", path.display()))?;

        info!("Loaded a config file `{}`", path.display());

        return Ok(cfg);
    }

    info!("Using the default config");

    Ok(Default::default())
}
