use std::fmt::{self, Display};

use handlebars::Handlebars;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    Index,
}

impl Template {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Index => "index",
        }
    }
}

impl Display for Template {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

pub fn new() -> Handlebars<'static> {
    let mut tt = Handlebars::new();
    tt.register_template_string(
        Template::Index.as_str(),
        include_str!("template/index.hbs"),
    )
    .unwrap();

    tt
}
