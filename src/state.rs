use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use handlebars::Handlebars;
use reqwest::Url;
use tokio::sync::Notify;

use crate::config::{self, Config, ExtractorConfig};
use crate::extractor::{Extractor, LuaExtractor, XPathExtractor};
use crate::storage::Storage;
use crate::template;

#[derive(Clone)]
pub struct State {
    pub storage: Arc<Storage>,
    pub cfg: Arc<Config>,
    pub feeds: Arc<HashMap<String, Feed>>,
    pub template: Arc<Handlebars<'static>>,
}

impl State {
    pub async fn new(cfg: Config) -> Result<Self> {
        let storage = Arc::new(Storage::new(&cfg.db_path).await?);
        let feeds = Arc::new(Self::make_feeds(&cfg)?);
        let cfg = Arc::new(cfg);
        let template = Arc::new(template::new());

        Ok(State {
            storage,
            cfg,
            feeds,
            template,
        })
    }

    fn make_feeds(cfg: &Config) -> Result<HashMap<String, Feed>> {
        cfg.feeds
            .iter()
            .map(|(name, feed)| Feed::new(cfg, feed).map(|feed| (name.clone(), feed)))
            .collect()
    }
}

pub struct Feed {
    pub request_url: Url,
    pub extractor: Mutex<Box<dyn Extractor + Send>>,
    pub fetch_interval: Duration,
    pub enabled: bool,
    pub force_update: Option<Arc<Notify>>,
}

impl Feed {
    fn new(cfg: &Config, feed: &config::Feed) -> Result<Self> {
        let fetch_interval = feed.fetch_interval.unwrap_or(cfg.fetch_interval).into();
        let extractor = Mutex::new(make_extractor(&feed.extractor)?);

        Ok(Feed {
            request_url: feed.request_url.clone(),
            extractor,
            fetch_interval,
            enabled: feed.enabled,
            force_update: feed.enabled.then(|| Arc::new(Notify::new())),
        })
    }
}

fn make_extractor(cfg: &ExtractorConfig) -> Result<Box<dyn Extractor + Send>> {
    Ok(match cfg {
        ExtractorConfig::XPath(cfg) => Box::new(XPathExtractor::from_cfg(cfg)),
        ExtractorConfig::Lua(cfg) => Box::new(LuaExtractor::from_cfg(cfg)?),
    })
}
