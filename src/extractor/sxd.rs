use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use anyhow::{bail, Context as _, Result};
use derive_more::From;
use elsa::FrozenVec;
use html5ever::interface::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tree_builder::TreeBuilderOpts;
use html5ever::{parse_document, Attribute, ExpandedName, ParseOpts, QualName};
use reqwest::Url;
use sxd_document::dom::{
    ChildOfElement, ChildOfRoot, Comment, Document, Element, ParentOfChild, ProcessingInstruction,
    Root, Text,
};
use sxd_document::{Package, QName};
use sxd_xpath::{Context, Value};
use tracing::{debug, warn};

use crate::config;
use crate::xpath::XPath;

use super::{Entry, Extractor};

const HTTP_XMLNS_URI: &str = "http://www.w3.org/1999/xhtml";

#[derive(Default)]
struct SxdSinkStorage {
    pkgs: FrozenVec<Box<Package>>,
}

impl SxdSinkStorage {
    fn document(&self) -> Document<'_> {
        self.pkgs[0].as_document()
    }

    fn into_package(mut self) -> Package {
        *self.pkgs.as_mut().remove(0)
    }
}

struct SxdSink<'s> {
    storage: &'s SxdSinkStorage,
    names: HashMap<(Option<&'s str>, &'s str), QualName>,
    template_contents: HashMap<Element<'s>, usize>,
    mathml_annotation_xml_integration_points: HashSet<Element<'s>>,
}

impl<'s> SxdSink<'s> {
    fn new(storage: &'s SxdSinkStorage) -> Self {
        storage.pkgs.push(Box::new(Package::new()));

        Self {
            storage,
            names: Default::default(),
            template_contents: Default::default(),
            mathml_annotation_xml_integration_points: Default::default(),
        }
    }

    fn get_name(&self, name: &QName<'s>) -> Option<&QualName> {
        self.names.get(&(name.namespace_uri(), name.local_part()))
    }

    fn intern_name(&mut self, name: QName<'s>) -> &QualName {
        self.names
            .entry((name.namespace_uri(), name.local_part()))
            .or_insert_with(|| QualName {
                prefix: None,
                ns: name.namespace_uri().unwrap_or("").into(),
                local: name.local_part().into(),
            })
    }
}

fn qual_name_to_qname(qual_name: &QualName) -> QName<'_> {
    if qual_name.ns.is_empty() {
        QName::new(&qual_name.local)
    } else {
        QName::with_namespace_uri(Some(&*qual_name.ns), &qual_name.local)
    }
}

fn set_default_namespace(handle: SxdHandle<'_>) {
    if let SxdHandle::Element(element) = handle {
        element.register_prefix("html", HTTP_XMLNS_URI);
        element.set_default_namespace_uri(Some(HTTP_XMLNS_URI));
    }
}

#[derive(From, Clone, Copy, PartialEq, Eq)]
enum SxdHandle<'s> {
    Root(Root<'s>),
    Comment(Comment<'s>),
    Element(Element<'s>),
    PI(ProcessingInstruction<'s>),
    Text(Text<'s>),
}

impl<'s> SxdHandle<'s> {
    fn parent(&self) -> Option<ParentOfChild<'s>> {
        match self {
            Self::Root(_) => None,
            Self::Comment(comment) => comment.parent(),
            Self::Element(element) => element.parent(),
            Self::PI(pi) => pi.parent(),
            Self::Text(txt) => txt.parent().map(ParentOfChild::Element),
        }
    }

    fn preceding_siblings(&self) -> Option<Vec<ChildOfElement<'s>>> {
        match self {
            Self::Root(_) => None,
            Self::Comment(comment) => Some(comment.preceding_siblings()),
            Self::Element(element) => Some(element.preceding_siblings()),
            Self::PI(pi) => Some(pi.preceding_siblings()),
            Self::Text(txt) => Some(txt.preceding_siblings()),
        }
    }

    fn following_siblings(&self) -> Option<Vec<ChildOfElement<'s>>> {
        match self {
            Self::Root(_) => None,
            Self::Comment(comment) => Some(comment.following_siblings()),
            Self::Element(element) => Some(element.following_siblings()),
            Self::PI(pi) => Some(pi.following_siblings()),
            Self::Text(txt) => Some(txt.following_siblings()),
        }
    }

    fn document(&self) -> Document<'s> {
        match self {
            Self::Root(root) => root.document(),
            Self::Comment(comment) => comment.document(),
            Self::Element(element) => element.document(),
            Self::PI(pi) => pi.document(),
            Self::Text(txt) => txt.document(),
        }
    }

    fn remove_from_parent(&self) {
        match self {
            Self::Root(_) => {}
            Self::Comment(comment) => comment.remove_from_parent(),
            Self::Element(element) => element.remove_from_parent(),
            Self::PI(pi) => pi.remove_from_parent(),
            Self::Text(txt) => txt.remove_from_parent(),
        }
    }

    fn children(&self) -> Option<Vec<ChildOfElement<'s>>> {
        match self {
            Self::Root(root) => Some(
                root.children()
                    .into_iter()
                    .map(ChildOfElement::from)
                    .collect(),
            ),
            Self::Comment(_) => None,
            Self::Element(element) => Some(element.children()),
            Self::PI(_) => None,
            Self::Text(_) => None,
        }
    }

    fn clear_children(&self) {
        match self {
            Self::Root(root) => root.clear_children(),
            Self::Comment(_) => {}
            Self::Element(element) => element.clear_children(),
            Self::PI(_) => {}
            Self::Text(_) => {}
        }
    }
}

impl<'s> From<ParentOfChild<'s>> for SxdHandle<'s> {
    fn from(parent: ParentOfChild<'s>) -> Self {
        match parent {
            ParentOfChild::Element(element) => element.into(),
            ParentOfChild::Root(root) => root.into(),
        }
    }
}

impl<'s> From<ChildOfElement<'s>> for SxdHandle<'s> {
    fn from(child: ChildOfElement<'s>) -> Self {
        match child {
            ChildOfElement::Element(element) => element.into(),
            ChildOfElement::Text(txt) => txt.into(),
            ChildOfElement::Comment(comment) => comment.into(),
            ChildOfElement::ProcessingInstruction(pi) => pi.into(),
        }
    }
}

impl<'s> From<ChildOfRoot<'s>> for SxdHandle<'s> {
    fn from(child: ChildOfRoot<'s>) -> Self {
        match child {
            ChildOfRoot::Element(element) => element.into(),
            ChildOfRoot::Comment(comment) => comment.into(),
            ChildOfRoot::ProcessingInstruction(pi) => pi.into(),
        }
    }
}

impl<'s> TryFrom<SxdHandle<'s>> for ChildOfRoot<'s> {
    type Error = &'static str;

    fn try_from(handle: SxdHandle<'s>) -> Result<Self, Self::Error> {
        match handle {
            SxdHandle::Root(_) => Err("a root node cannot be a child of a root node"),
            SxdHandle::Comment(comment) => Ok(ChildOfRoot::Comment(comment)),
            SxdHandle::Element(element) => Ok(ChildOfRoot::Element(element)),
            SxdHandle::PI(pi) => Ok(ChildOfRoot::ProcessingInstruction(pi)),
            SxdHandle::Text(_) => Err("a text node cannot be a child of a root node"),
        }
    }
}

impl<'s> TryFrom<SxdHandle<'s>> for ChildOfElement<'s> {
    type Error = &'static str;

    fn try_from(handle: SxdHandle<'s>) -> Result<Self, Self::Error> {
        match handle {
            SxdHandle::Root(_) => Err("a root node cannot be a child of a node"),
            SxdHandle::Comment(comment) => Ok(ChildOfElement::Comment(comment)),
            SxdHandle::Element(element) => Ok(ChildOfElement::Element(element)),
            SxdHandle::PI(pi) => Ok(ChildOfElement::ProcessingInstruction(pi)),
            SxdHandle::Text(txt) => Ok(ChildOfElement::Text(txt)),
        }
    }
}

impl<'s> TreeSink for SxdSink<'s> {
    type Handle = SxdHandle<'s>;
    type Output = ();

    fn finish(self) {}

    fn parse_error(&mut self, msg: Cow<'static, str>) {
        debug!("Encountered an HTML parsing error: {msg}");
    }

    fn get_document(&mut self) -> Self::Handle {
        self.storage.document().root().into()
    }

    fn elem_name<'a>(&'a self, target: &'a Self::Handle) -> ExpandedName<'a> {
        match target {
            SxdHandle::Element(elem) => self
                .get_name(&elem.name())
                .expect("element name is not saved")
                .expanded(),

            _ => panic!("the target is not an element and has no name"),
        }
    }

    fn create_element(
        &mut self,
        name: QualName,
        attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> Self::Handle {
        let element = self
            .storage
            .document()
            .create_element(qual_name_to_qname(&name));
        self.intern_name(element.name());

        if let Some(prefix) = &name.prefix {
            element.register_prefix(prefix, &name.ns);
        }

        if flags.template {
            let pkg_id = self.storage.pkgs.len();
            self.storage.pkgs.push(Box::new(Package::new()));
            self.template_contents.insert(element, pkg_id);
        }

        if flags.mathml_annotation_xml_integration_point {
            self.mathml_annotation_xml_integration_points
                .insert(element);
        }

        for attr in attrs {
            let attr = element.set_attribute_value(qual_name_to_qname(&attr.name), &attr.value);
            self.intern_name(attr.name());
        }

        element.into()
    }

    fn create_comment(&mut self, text: StrTendril) -> Self::Handle {
        self.storage.document().create_comment(&text).into()
    }

    fn create_pi(&mut self, target: StrTendril, data: StrTendril) -> Self::Handle {
        self.storage
            .document()
            .create_processing_instruction(&target, Some(&*data))
            .into()
    }

    fn append(&mut self, parent: &Self::Handle, child: NodeOrText<Self::Handle>) {
        let handle = match child {
            NodeOrText::AppendNode(handle) => handle,

            NodeOrText::AppendText(s) => {
                match parent {
                    SxdHandle::Root(_) => panic!("appending a text child to the root"),

                    SxdHandle::Element(element) => {
                        if let Some(ChildOfElement::Text(text)) = element.children().last() {
                            text.set_text(&format!("{}{s}", text.text()));

                            return;
                        }
                    }

                    _ => {}
                }

                self.storage.document().create_text(&s).into()
            }
        };

        match parent {
            SxdHandle::Root(root) => {
                set_default_namespace(handle);
                root.append_child(ChildOfRoot::try_from(handle).unwrap());
            }

            SxdHandle::Comment(_) => panic!("appending a child to a comment node"),
            SxdHandle::Element(element) => {
                element.append_child(ChildOfElement::try_from(handle).unwrap())
            }
            SxdHandle::PI(_) => panic!("appending a child to a processing instruction node"),
            SxdHandle::Text(_) => panic!("appending a child to a text node"),
        }
    }

    fn append_based_on_parent_node(
        &mut self,
        element: &Self::Handle,
        prev_element: &Self::Handle,
        child: html5ever::interface::NodeOrText<Self::Handle>,
    ) {
        if element.parent().is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &mut self,
        _name: StrTendril,
        _public_id: StrTendril,
        _system_id: StrTendril,
    ) {
        // ignore. sxd_document doesn't have DOCTYPEs.
    }

    fn get_template_contents(&mut self, target: &Self::Handle) -> Self::Handle {
        let SxdHandle::Element(element) = target else {
            panic!("template contents can only be associated with element nodes");
        };
        let doc_id = *self
            .template_contents
            .get(element)
            .expect("no template contents associated with an element node");

        self.storage.pkgs[doc_id].as_document().root().into()
    }

    fn same_node(&self, x: &Self::Handle, y: &Self::Handle) -> bool {
        x == y
    }

    fn set_quirks_mode(&mut self, _mode: QuirksMode) {
        // ignore. we don't care about quirks.
    }

    fn append_before_sibling(
        &mut self,
        sibling: &Self::Handle,
        new_node: NodeOrText<Self::Handle>,
    ) {
        let parent = sibling.parent().expect("the sibling must have a parent");

        match parent {
            ParentOfChild::Root(root) => {
                let mut children = sibling.preceding_siblings().unwrap();

                children.push(match new_node {
                    NodeOrText::AppendNode(handle) => {
                        set_default_namespace(handle);

                        handle.try_into().unwrap()
                    }

                    NodeOrText::AppendText(_) => {
                        panic!("trying to add a text node as a child of the root node")
                    }
                });

                children.push(TryFrom::try_from(*sibling).unwrap());
                children.extend(sibling.following_siblings().unwrap());
                let children = children
                    .into_iter()
                    .map(|child| ChildOfRoot::try_from(SxdHandle::from(child)).unwrap());
                root.replace_children(children);
            }

            ParentOfChild::Element(element) => {
                let mut children = sibling.preceding_siblings().unwrap();

                if let (Some(&ChildOfElement::Text(txt)), NodeOrText::AppendText(s)) =
                    (children.last(), &new_node)
                {
                    txt.set_text(&format!("{}{s}", txt.text()));

                    return;
                }

                children.push(match new_node {
                    NodeOrText::AppendNode(handle) => handle.try_into().unwrap(),
                    NodeOrText::AppendText(s) => sibling.document().create_text(&s).into(),
                });
                children.extend(sibling.following_siblings().unwrap());
                element.replace_children(children);
            }
        }
    }

    fn add_attrs_if_missing(&mut self, target: &Self::Handle, attrs: Vec<Attribute>) {
        let SxdHandle::Element(element) = *target else {
            panic!("trying to add attributes to a non-element node");
        };

        for attr in attrs {
            let qname = qual_name_to_qname(&attr.name);

            if element.attribute_value(qname).is_none() {
                let attr = element.set_attribute_value(qname, &attr.value);
                self.intern_name(attr.name());
            }
        }
    }

    fn remove_from_parent(&mut self, target: &Self::Handle) {
        target.remove_from_parent();
    }

    fn reparent_children(&mut self, node: &Self::Handle, new_parent: &Self::Handle) {
        let children = node
            .children()
            .expect("the source node must be a root or an element node");
        node.clear_children();

        match new_parent {
            SxdHandle::Root(root) => {
                let children = children
                    .into_iter()
                    .map(SxdHandle::from)
                    .inspect(|handle| set_default_namespace(*handle))
                    .map(|child| ChildOfRoot::try_from(child).unwrap());
                root.append_children(children);
            }

            SxdHandle::Element(element) => element.append_children(children),

            _ => panic!("the new parent must be a root or an element node"),
        }
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &Self::Handle) -> bool {
        if let SxdHandle::Element(element) = handle {
            self.mathml_annotation_xml_integration_points
                .contains(element)
        } else {
            false
        }
    }
}

fn parse_html(html: &str) -> Package {
    let storage = SxdSinkStorage::default();

    parse_document(
        SxdSink::new(&storage),
        ParseOpts {
            tree_builder: TreeBuilderOpts {
                scripting_enabled: false,
                iframe_srcdoc: false,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .one(html);

    storage.into_package()
}

fn xpath_value_to_string(value: Value<'_>) -> String {
    if let Value::Nodeset(nodes) = value {
        // concatenate all nodes
        let mut s = String::new();

        for node in nodes.document_order() {
            s.push_str(&node.string_value());
        }

        s
    } else {
        value.into_string()
    }
}

pub struct XPathExtractor {
    entry: XPath,
    id: XPath,
    title: XPath,
    description: XPath,
    url: XPath,
    author: Option<XPath>,
}

impl XPathExtractor {
    pub fn from_cfg(cfg: &config::XPathExtractorConfig) -> Self {
        Self {
            entry: cfg.entry.clone(),
            id: cfg.id.clone(),
            title: cfg.title.clone(),
            description: cfg.description.clone(),
            url: cfg.url.clone(),
            author: cfg.author.clone(),
        }
    }
}

impl Extractor for XPathExtractor {
    fn extract(&mut self, html: &str) -> Result<Vec<Entry>> {
        let html = parse_html(html);
        let mut xpath_ctx = Context::new();
        xpath_ctx.set_namespace("html", HTTP_XMLNS_URI);
        xpath_ctx.set_default_namespace_uri(Some(HTTP_XMLNS_URI.into()));

        let entries = self
            .entry
            .evaluate(&xpath_ctx, html.as_document().root())
            .context("could not apply the entry XPath expression")?;
        let entries = 'entries: {
            let expected = match entries {
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Boolean(_) => "boolean",
                Value::Nodeset(nodes) => break 'entries nodes,
            };

            bail!("the entry XPath expression returned a {expected} instead of a node set");
        };

        let mut result = vec![];

        for (idx, entry) in entries.document_order().into_iter().enumerate() {
            let idx = idx + 1;

            let find_one = |xpath: &XPath, what: &str, allow_empty: bool| {
                let value = match xpath.evaluate(&xpath_ctx, entry) {
                    Ok(value) => value,

                    Err(e) => {
                        warn!("Could not apply the {what} XPath expression to entry #{idx}: {e:#}");
                        return None;
                    }
                };

                let s = xpath_value_to_string(value);

                if s.is_empty() && !allow_empty {
                    warn!("The {what} XPath expression returned an empty string");

                    None
                } else {
                    Some(s)
                }
            };

            let Some(id) = find_one(&self.id, "id", false) else {
                continue;
            };
            let Some(title) = find_one(&self.title, "title", false) else {
                continue;
            };
            let Some(description) = find_one(&self.description, "description", true) else {
                continue;
            };
            let Some(url) = find_one(&self.url, "url", false) else {
                continue;
            };
            let url = match Url::parse(&url) {
                Ok(url) => url,
                Err(e) => {
                    warn!(
                        "The result of evaluating the url XPath expression for entry #{idx} \
                            could not be parsed as an URL: {e:#}",
                    );
                    continue;
                }
            };
            let author = self
                .author
                .as_ref()
                .and_then(|xpath| find_one(xpath, "author", false));

            result.push(Entry {
                id,
                title,
                description,
                url,
                author,

                // TODO
                pub_date: None,
            });
        }

        Ok(result)
    }
}
