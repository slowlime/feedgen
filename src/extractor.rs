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

pub trait Extractor {
    fn extract(&mut self, html: &str) -> Result<Vec<Entry>>;
}
