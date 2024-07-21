mod xpath;

use anyhow::Result;
use reqwest::Url;
use time::OffsetDateTime;

pub use xpath::XPathExtractor;

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub description: String,
    pub url: Url,
    pub author: Option<String>,
    pub pub_date: Option<OffsetDateTime>,
}

pub struct Context<'c> {
    fetch_url: &'c Url,
}

impl<'c> Context<'c> {
    pub fn new(fetch_url: &'c Url) -> Self {
        Self { fetch_url }
    }

    pub fn fetch_url(&self) -> &'c Url {
        self.fetch_url
    }
}

pub trait Extractor {
    fn extract<'c>(&mut self, ctx: Context<'c>, html: &str) -> Result<Vec<Entry>>;
}
