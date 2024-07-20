use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::fmt::Formatter;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context as _, Result};
use serde::de::Visitor;
use serde::{Deserialize, Deserializer};
use sxd_xpath::nodeset::Node;
use sxd_xpath::{Context, ExecutionError, Factory, Value};

static NEXT_XPATH_ID: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static XPATH_REGISTRY: RefCell<HashMap<usize, sxd_xpath::XPath>> = RefCell::new(HashMap::new());
}

#[derive(Debug, Clone)]
struct XPathInner {
    id: usize,
    s: String,
}

#[derive(Debug, Clone)]
pub struct XPath(Arc<XPathInner>);

impl XPath {
    pub fn new(s: String) -> Result<Self> {
        let xpath = Factory::new()
            .build(&s)
            .context("could not compile the XPath expression")?
            // what's going on with the API design here?
            .ok_or_else(|| anyhow!("no XPath expression was parsed"))?;

        let id = NEXT_XPATH_ID.fetch_add(1, Ordering::Relaxed);
        XPATH_REGISTRY.with_borrow_mut(|registry| {
            registry.insert(id, xpath);
        });

        Ok(XPath(Arc::new(XPathInner { id, s })))
    }

    pub fn with<R>(&self, f: impl FnOnce(&sxd_xpath::XPath) -> R) -> R {
        XPATH_REGISTRY.with_borrow_mut(|registry| {
            f(registry
                .entry(self.0.id)
                .or_insert_with(|| Factory::new().build(&self.0.s).unwrap().unwrap()))
        })
    }

    pub fn evaluate<'d, N>(
        &self,
        context: &Context<'d>,
        node: N,
    ) -> Result<Value<'d>, ExecutionError>
    where
        N: Into<Node<'d>>,
    {
        self.with(|xpath| xpath.evaluate(context, node))
    }
}

impl<'de> Deserialize<'de> for XPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct XPathVisitor;

        impl<'de> Visitor<'de> for XPathVisitor {
            type Value = XPath;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
                write!(formatter, "an XPath expression")
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                XPath::new(v).map_err(E::custom)
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                self.visit_string(v.into())
            }
        }

        deserializer.deserialize_string(XPathVisitor)
    }
}
