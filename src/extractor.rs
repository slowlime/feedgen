use anyhow::{bail, Context, Result};
use reqwest::Url;
use skyscraper::xpath::grammar::data_model::{Node, XpathItem};
use skyscraper::xpath::grammar::NonTreeXpathNode;
use skyscraper::xpath::{Xpath, XpathItemTree};
use time::OffsetDateTime;
use tracing::warn;

use crate::config;

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

pub struct XpathExtractor {
    entry: Xpath,
    id: Xpath,
    title: Xpath,
    description: Xpath,
    url: Xpath,
    author: Option<Xpath>,
}

impl XpathExtractor {
    pub fn from_cfg(cfg: &config::XpathExtractorConfig) -> Self {
        Self {
            entry: cfg.entry.compile(),
            id: cfg.id.compile(),
            title: cfg.title.compile(),
            description: cfg.description.compile(),
            url: cfg.url.compile(),
            author: cfg.author.as_ref().map(|xpath| xpath.compile()),
        }
    }

    fn item_to_string(item: &XpathItem<'_>, tree: &XpathItemTree) -> Result<String> {
        Ok(match item {
            XpathItem::Node(Node::TreeNode(node)) => node.text(tree).unwrap_or_default(),

            XpathItem::Node(Node::NonTreeNode(node)) => match node {
                NonTreeXpathNode::AttributeNode(node) => node.value.clone(),
                NonTreeXpathNode::NamespaceNode(_) => bail!("the XPath item is a namespace node"),
            },

            XpathItem::Function(_) => bail!("the XPath item is a function"),

            XpathItem::AnyAtomicType(value) => value.to_string(),
        })
    }
}

impl Extractor for XpathExtractor {
    fn extract(&mut self, html: &str) -> Result<Vec<Entry>> {
        let html = skyscraper::html::parse(html).context("could not parse the HTML document")?;
        let item_tree = XpathItemTree::from(&html);
        let mut entries = vec![];

        for (idx, entry) in self
            .entry
            .apply(&item_tree)
            .context("could not apply the entry XPath expression")?
            .into_iter()
            .enumerate()
        {
            let idx = idx + 1;

            let find_one = |xpath: &Xpath, what: &str| {
                let Ok(items) = xpath.apply_to_item(&item_tree, entry.clone()) else {
                    warn!("Could not apply the {what} XPath expression to entry #{idx}");
                    return None;
                };

                let item = match items.len() {
                    0 => {
                        warn!(
                            "The {what} XPath expression did not return \
                                any results for entry #{idx}",
                        );
                        return None;
                    }

                    1 => &items[0],

                    n => {
                        warn!(
                            "The {what} XPath expression returned {n} results for entry #{idx}; \
                                choosing the first one",
                        );

                        &items[0]
                    }
                };

                match Self::item_to_string(item, &item_tree) {
                    Ok(result) => Some(result),

                    Err(e) => {
                        warn!(
                            "The result of evaluating the {what} XPath expression for entry #{idx} \
                                could not be converted to string: {e}",
                        );

                        None
                    }
                }
            };

            let Some(id) = find_one(&self.id, "id") else {
                continue;
            };
            let Some(title) = find_one(&self.title, "title") else {
                continue;
            };
            let Some(description) = find_one(&self.description, "description") else {
                continue;
            };
            let Some(url) = find_one(&self.url, "url") else {
                continue;
            };
            let url = match Url::parse(&url) {
                Ok(url) => url,
                Err(e) => {
                    warn!(
                        "The result of evaluating the url XPath expression for entry #{idx} \
                            could not be parsed as an URL: {e}",
                    );
                    continue;
                }
            };
            let author = self
                .author
                .as_ref()
                .and_then(|xpath| find_one(xpath, "author"));

            entries.push(Entry {
                id,
                title,
                description,
                url,
                author,

                // TODO
                pub_date: None,
            });
        }

        Ok(entries)
    }
}
