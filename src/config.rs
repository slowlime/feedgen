use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use regex_lite::Regex;
use reqwest::Url;
use serde::de::Visitor;
use serde::{Deserialize, Deserializer};
use tracing::{debug, info};

use crate::xpath::XPath;

fn default_fetch_interval() -> Duration {
    Config::default().fetch_interval
}

fn default_max_initial_fetch_sleep() -> Duration {
    Config::default().max_initial_fetch_sleep
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
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

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct Feed {
    pub request_url: Url,
    pub extractor: ExtractorConfig,
    pub fetch_interval: Option<Duration>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ExtractorConfig {
    #[serde(rename = "xpath")]
    XPath(XPathExtractorConfig),
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct XPathExtractorConfig {
    pub entry: XPath,
    pub id: XPath,
    pub title: XPath,
    pub description: XPath,
    pub url: XPath,
    pub author: Option<XPath>,
}

#[derive(Debug, Clone, Copy)]
pub struct Duration(std::time::Duration);

impl Duration {
    pub fn from_secs(seconds: u64) -> Self {
        Self(std::time::Duration::from_secs(seconds))
    }
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DurationVisitor;

        impl<'de> Visitor<'de> for DurationVisitor {
            type Value = Duration;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                write!(formatter, "a duration")
            }

            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                self.visit_u64(v.try_into().map_err(E::custom)?)
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(Duration::from_secs(v))
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                use serde::de::Unexpected;

                static REGEXP: OnceLock<Regex> = OnceLock::new();

                let regexp = REGEXP.get_or_init(|| {
                    Regex::new(
                        r"^(?<days>:(\d+)d)?\s*\
                        (?<hours>:(\d+)h)?\s*\
                        (?<minutes>:(\d+)m)?\s*\
                        (?<seconds>:(\d+)s)?$",
                    )
                    .unwrap()
                });
                let Some(captures) = regexp.captures(v) else {
                    return Err(E::invalid_value(Unexpected::Str(v), &"a duration"));
                };

                let parse = |name: &str| {
                    let s = &captures[name];

                    if s.is_empty() {
                        Ok(None)
                    } else {
                        s.parse::<u64>()
                            .map(Some)
                            .map_err(|e| E::custom(format!("could not parse {name} (`{s}`): {e}")))
                    }
                };

                let days = parse("days")?;
                let hours = parse("hours")?;
                let minutes = parse("minutes")?;
                let seconds = parse("seconds")?;

                if days.is_none() && hours.is_none() && minutes.is_none() && seconds.is_none() {
                    return Err(E::invalid_value(Unexpected::Str(v), &"a duration"));
                }

                days.unwrap_or(0)
                    .checked_mul(24)
                    .and_then(|h| h.checked_add(hours.unwrap_or(0)))
                    .and_then(|h| h.checked_mul(60))
                    .and_then(|m| m.checked_add(minutes.unwrap_or(0)))
                    .and_then(|m| m.checked_mul(60))
                    .and_then(|s| s.checked_add(seconds.unwrap_or(0)))
                    .map(Duration::from_secs)
                    .ok_or_else(|| E::custom(format!("duration `{v}` is too large")))
            }
        }

        deserializer.deserialize_str(DurationVisitor)
    }
}

impl From<std::time::Duration> for Duration {
    fn from(duration: std::time::Duration) -> Self {
        Self(duration)
    }
}

impl From<Duration> for std::time::Duration {
    fn from(duration: Duration) -> Self {
        duration.0
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
