use std::ops::Deref;
use std::sync::Arc;

use derive_more::From;
use ego_tree::iter::{Children, Descendants};
use ego_tree::{NodeId, NodeRef};
use mlua::prelude::*;
use ouroboros::self_referencing;
use scraper::node::{Attrs, Classes, Comment, Doctype, ProcessingInstruction, Text};
use scraper::selector::ToCss;
use scraper::{element_ref, Node};
use scraper::{CaseSensitivity, ElementRef, Html, Selector};
use time::{Date, Month, OffsetDateTime, Time, UtcOffset};
use time_tz::{timezones, OffsetResult, PrimitiveDateTimeExt};
use tracing::warn;

#[derive(From, Clone)]
#[from(forward)]
pub struct Buffer(Arc<str>);

impl Buffer {
    fn to_string(_lua: &Lua, this: &Self, _: ()) -> LuaResult<String> {
        Ok(this.0.to_string())
    }

    fn len(_lua: &Lua, this: &Self, _: ()) -> LuaResult<usize> {
        Ok(this.0.len())
    }
}

impl FromLua<'_> for Buffer {
    fn from_lua(value: LuaValue<'_>, _lua: &Lua) -> LuaResult<Self> {
        match value {
            LuaValue::UserData(ud) => ud.borrow::<Self>().map(|this| this.clone()),
            LuaValue::String(s) => Ok(Buffer(s.to_str()?.into())),

            _ => Err(LuaError::FromLuaConversionError {
                from: value.type_name(),
                to: "Buffer",
                message: Some("expected string or Buffer".into()),
            }),
        }
    }
}

impl LuaUserData for Buffer {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_meta_method("__tostring", Self::to_string);
        methods.add_meta_method("__len", Self::len);
    }
}

impl Deref for Buffer {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

struct Stringified(String);

impl Deref for Stringified {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'lua> FromLua<'lua> for Stringified {
    fn from_lua(value: LuaValue<'lua>, _lua: &'lua Lua) -> LuaResult<Self> {
        if value.is_number() || value.is_string() {
            Ok(Self(value.to_string()?))
        } else if let Some(to_string) = match &value {
            LuaValue::Table(tbl) => {
                if let Some(mt) = tbl.get_metatable() {
                    mt.raw_get::<_, Option<LuaFunction<'lua>>>("__tostring")?
                } else {
                    None
                }
            }

            LuaValue::UserData(ud) => ud.get_metatable()?.get("__tostring")?,

            _ => None,
        } {
            let result: LuaValue<'lua> = to_string.call(value)?;

            if let Some(s) = result.as_str() {
                Ok(Self(s.into()))
            } else {
                Err(LuaError::runtime("'__tostring' must return a string"))
            }
        } else {
            Err(LuaError::FromLuaConversionError {
                from: value.type_name(),
                to: "string",
                message: Some(
                    "expected string, number, or table/userdata with __tostring metamethod".into(),
                ),
            })
        }
    }
}

struct NonEmptyString(String);

impl<'lua> FromLua<'lua> for NonEmptyString {
    fn from_lua(value: LuaValue<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        let s = Stringified::from_lua(value, lua)?;

        if s.is_empty() {
            Err(LuaError::runtime("string must be non-empty"))
        } else {
            Ok(Self(s.0))
        }
    }
}

struct PubDate(OffsetDateTime);

impl<'lua> FromLua<'lua> for PubDate {
    fn from_lua(value: LuaValue<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        let tbl = LuaTable::from_lua(value, lua)?;
        let year: i32 = tbl.get("year")?;
        let month: u8 = tbl.get("month")?;
        let day: u8 = tbl.get("day")?;
        let hour: u8 = tbl.get("hour")?;
        let minute: u8 = tbl.get("minute")?;
        let second: u8 = tbl.get("second")?;
        let utc_offset: Option<i16> = tbl.get("utcOffset")?;
        let tz: Option<NonEmptyString> = tbl.get("tz")?;

        let month = Month::try_from(month)
            .map_err(|e| LuaError::runtime(format!("month {month} is invalid: {e}")))?;
        let date = Date::from_calendar_date(year, month, day).map_err(|e| {
            LuaError::runtime(format!("date {year}-{}-{day} is invalid: {e}", month as u8))
        })?;
        let time = Time::from_hms(hour, minute, second).map_err(|e| {
            LuaError::runtime(format!("time {hour}:{minute}:{second} is invalid: {e}"))
        })?;
        let datetime = date.with_time(time);

        if let Some(name) = tz {
            let name = name.0;
            let tz = timezones::get_by_name(&name)
                .ok_or_else(|| LuaError::runtime(format_args!("unknown timezone '{name}'")))?;

            match datetime.assume_timezone(tz) {
                OffsetResult::Some(dt) => Ok(Self(dt)),

                OffsetResult::Ambiguous(lhs, rhs) => {
                    warn!(
                        "Datetime {datetime} is ambiguous in the timezone `{name}`: \
                            could be {lhs} or {rhs}; picking the former"
                    );

                    Ok(Self(lhs))
                }

                OffsetResult::None => Err(LuaError::runtime(format!(
                    "datetime {datetime} is invalid in timezone '{name}'"
                ))),
            }
        } else if let Some(whole_minutes) = utc_offset {
            let hours: i8 = whole_minutes.div_euclid(60).try_into().map_err(|_| {
                LuaError::runtime(format!("UTC offset {whole_minutes} is too large"))
            })?;
            let minutes = whole_minutes.rem_euclid(60) as i8;
            let utc_offset = UtcOffset::from_hms(hours, minutes, 0).map_err(|e| {
                LuaError::runtime(format!("UTC offset {whole_minutes} is invalid: {e}"))
            })?;

            Ok(Self(datetime.assume_offset(utc_offset)))
        } else {
            Err(LuaError::runtime(
                "neither 'tz' nor 'utcOffset' was specified",
            ))
        }
    }
}

pub struct LuaEntry {
    pub id: String,
    pub title: String,
    pub description: String,
    pub url: String,
    pub author: Option<String>,
    pub pub_date: Option<OffsetDateTime>,
}

impl<'lua> FromLua<'lua> for LuaEntry {
    fn from_lua(value: LuaValue<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        let entry = LuaTable::from_lua(value, lua)?;
        let id: NonEmptyString = entry.get("id")?;
        let title: NonEmptyString = entry.get("title")?;
        let description: Stringified = entry.get("description")?;
        let url: Stringified = entry.get("url")?;
        let author: Option<Stringified> = entry.get("author")?;
        let pub_date: Option<PubDate> = entry.get("pubDate")?;

        Ok(LuaEntry {
            id: id.0,
            title: title.0,
            description: description.0,
            url: url.0,
            author: author
                .map(|author| author.0)
                .filter(|author| !author.is_empty()),
            pub_date: pub_date.map(|pub_date| pub_date.0),
        })
    }
}

#[derive(From, Clone)]
pub struct SelectorWrapper(Arc<Selector>);

impl SelectorWrapper {
    fn to_string(_lua: &Lua, this: &Self, _: ()) -> LuaResult<String> {
        Ok(this.0.to_css_string())
    }
}

impl FromLua<'_> for SelectorWrapper {
    fn from_lua(value: LuaValue<'_>, _lua: &Lua) -> LuaResult<Self> {
        match value {
            LuaValue::UserData(ud) => ud.borrow::<Self>().map(|this| this.clone()),

            LuaValue::String(s) => Ok(Self(Arc::new(Selector::parse(s.to_str()?).map_err(
                |e| LuaError::runtime(format_args!("could not parse the CSS selector: {e}")),
            )?))),

            _ => Err(LuaError::FromLuaConversionError {
                from: value.type_name(),
                to: "Selector",
                message: Some("expected string or Selector".into()),
            }),
        }
    }
}

impl LuaUserData for SelectorWrapper {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_meta_method("__tostring", Self::to_string);
    }
}

#[derive(From, Clone)]
#[from(forward)]
pub struct LuaHtml(Arc<Html>);

impl LuaHtml {
    fn select(_lua: &Lua, this: &Self, selector: SelectorWrapper) -> LuaResult<LuaHtmlSelect> {
        Ok(LuaHtmlSelect::new(
            this.0.clone(),
            selector.0,
            |html, selector| html.select(selector),
        ))
    }

    fn root(_lua: &Lua, this: &Self, _: ()) -> LuaResult<LuaElementRef> {
        Ok(LuaElementRef::new(this.0.clone(), |html| {
            html.root_element()
        }))
    }
}

impl LuaUserData for LuaHtml {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("select", Self::select);
        methods.add_method("root", Self::root);
    }
}

#[self_referencing]
struct LuaHtmlSelect {
    html: Arc<Html>,
    selector: Arc<Selector>,

    #[borrows(html, selector)]
    #[covariant]
    select: scraper::html::Select<'this, 'this>,
}

impl LuaHtmlSelect {
    fn call(_lua: &Lua, this: &mut Self, _: ()) -> LuaResult<Option<LuaElementRef>> {
        Ok(this.with_mut(|fields| {
            fields.select.next().map(|element| {
                LuaElementRef::from_node_id(fields.html.clone(), element.id()).unwrap()
            })
        }))
    }
}

impl LuaUserData for LuaHtmlSelect {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_meta_method_mut("__call", Self::call);
    }
}

enum BaseNodeRef {
    Node(LuaNodeRef),
    Doctype(LuaDoctypeRef),
    Comment(LuaCommentRef),
    Text(LuaTextRef),
    Element(LuaElementRef),
    ProcessingInstruction(LuaProcessingInstructionRef),
}

trait IntoBaseNodeRef: 'static {
    fn html(&self) -> Arc<Html>;

    fn as_node_ref(&self) -> NodeRef<'_, Node>;
}

impl IntoBaseNodeRef for BaseNodeRef {
    fn html(&self) -> Arc<Html> {
        match self {
            Self::Node(r) => r.html(),
            Self::Doctype(r) => r.html(),
            Self::Comment(r) => r.html(),
            Self::Text(r) => r.html(),
            Self::Element(r) => r.html(),
            Self::ProcessingInstruction(r) => r.html(),
        }
    }

    fn as_node_ref(&self) -> NodeRef<'_, Node> {
        match self {
            Self::Node(r) => r.as_node_ref(),
            Self::Doctype(r) => r.as_node_ref(),
            Self::Comment(r) => r.as_node_ref(),
            Self::Text(r) => r.as_node_ref(),
            Self::Element(r) => r.as_node_ref(),
            Self::ProcessingInstruction(r) => r.as_node_ref(),
        }
    }
}

impl BaseNodeRef {
    fn from_node_ref(html: Arc<Html>, node_ref: NodeRef<'_, Node>) -> Self {
        let node_id = node_ref.id();

        match node_ref.value() {
            Node::Document | Node::Fragment => {
                Self::Node(LuaNodeRef::from_node_id(html, node_id).unwrap())
            }
            Node::Doctype(_) => Self::Doctype(LuaDoctypeRef::from_node_id(html, node_id).unwrap()),
            Node::Comment(_) => Self::Comment(LuaCommentRef::from_node_id(html, node_id).unwrap()),
            Node::Text(_) => Self::Text(LuaTextRef::from_node_id(html, node_id).unwrap()),
            Node::Element(_) => Self::Element(LuaElementRef::from_node_id(html, node_id).unwrap()),
            Node::ProcessingInstruction(_) => Self::ProcessingInstruction(
                LuaProcessingInstructionRef::from_node_id(html, node_id).unwrap(),
            ),
        }
    }

    fn add_methods<'lua, T, M>(methods: &mut M)
    where
        M: LuaUserDataMethods<'lua, T>,
        T: IntoBaseNodeRef,
    {
        methods.add_method("type", Self::type_);
        methods.add_method("parent", Self::parent);
        methods.add_method("prevSibling", Self::prev_sibling);
        methods.add_method("nextSibling", Self::next_sibling);
        methods.add_method("firstChildNode", Self::first_child_node);
        methods.add_method("lastChildNode", Self::last_child_node);
        methods.add_method("childNodes", Self::child_nodes);
        methods.add_method("descendantNodes", Self::descendant_nodes);
    }

    fn type_(_lua: &Lua, this: &impl IntoBaseNodeRef, _: ()) -> LuaResult<String> {
        Ok(match this.as_node_ref().value() {
            Node::Document => "document".into(),
            Node::Fragment => "fragment".into(),
            Node::Doctype(_) => "doctype".into(),
            Node::Comment(_) => "comment".into(),
            Node::Text(_) => "text".into(),
            Node::Element(_) => "element".into(),
            Node::ProcessingInstruction(_) => "processing instruction".into(),
        })
    }

    fn parent(_lua: &Lua, this: &impl IntoBaseNodeRef, _: ()) -> LuaResult<Option<BaseNodeRef>> {
        Ok(this
            .as_node_ref()
            .parent()
            .map(|node_ref| BaseNodeRef::from_node_ref(this.html(), node_ref)))
    }

    fn prev_sibling(
        _lua: &Lua,
        this: &impl IntoBaseNodeRef,
        _: (),
    ) -> LuaResult<Option<BaseNodeRef>> {
        Ok(this
            .as_node_ref()
            .prev_sibling()
            .map(|node_ref| BaseNodeRef::from_node_ref(this.html(), node_ref)))
    }

    fn next_sibling(
        _lua: &Lua,
        this: &impl IntoBaseNodeRef,
        _: (),
    ) -> LuaResult<Option<BaseNodeRef>> {
        Ok(this
            .as_node_ref()
            .next_sibling()
            .map(|node_ref| BaseNodeRef::from_node_ref(this.html(), node_ref)))
    }

    fn first_child_node(
        _lua: &Lua,
        this: &impl IntoBaseNodeRef,
        _: (),
    ) -> LuaResult<Option<BaseNodeRef>> {
        Ok(this
            .as_node_ref()
            .first_child()
            .map(|node_ref| BaseNodeRef::from_node_ref(this.html(), node_ref)))
    }

    fn last_child_node(
        _lua: &Lua,
        this: &impl IntoBaseNodeRef,
        _: (),
    ) -> LuaResult<Option<BaseNodeRef>> {
        Ok(this
            .as_node_ref()
            .last_child()
            .map(|node_ref| BaseNodeRef::from_node_ref(this.html(), node_ref)))
    }

    fn child_nodes(_lua: &Lua, this: &impl IntoBaseNodeRef, _: ()) -> LuaResult<LuaChildren> {
        let node_id = this.as_node_ref().id();

        Ok(LuaChildrenBuilder {
            html: this.html(),
            elements_only: false,
            iter_builder: |html| html.tree.get(node_id).unwrap().children(),
        }
        .build())
    }

    fn descendant_nodes(
        _lua: &Lua,
        this: &impl IntoBaseNodeRef,
        _: (),
    ) -> LuaResult<LuaDescendants> {
        let node_id = this.as_node_ref().id();

        Ok(LuaDescendantsBuilder {
            html: this.html(),
            elements_only: false,
            iter_builder: |html| html.tree.get(node_id).unwrap().descendants(),
        }
        .build())
    }
}

impl<'lua> IntoLua<'lua> for BaseNodeRef {
    fn into_lua(self, lua: &'lua Lua) -> LuaResult<LuaValue<'lua>> {
        match self {
            Self::Node(node_ref) => node_ref.into_lua(lua),
            Self::Doctype(doctype_ref) => doctype_ref.into_lua(lua),
            Self::Comment(comment_ref) => comment_ref.into_lua(lua),
            Self::Text(text_ref) => text_ref.into_lua(lua),
            Self::Element(element_ref) => element_ref.into_lua(lua),
            Self::ProcessingInstruction(pi_ref) => pi_ref.into_lua(lua),
        }
    }
}

#[self_referencing]
struct LuaChildren {
    html: Arc<Html>,
    elements_only: bool,

    #[borrows(html)]
    #[covariant]
    iter: Children<'this, Node>,
}

impl LuaChildren {
    fn call(_lua: &Lua, this: &mut Self, _: ()) -> LuaResult<Option<BaseNodeRef>> {
        Ok(this.with_mut(|fields| {
            fields
                .iter
                .next()
                .filter(|node_ref| !*fields.elements_only || node_ref.value().is_element())
                .map(|node_ref| BaseNodeRef::from_node_ref(fields.html.clone(), node_ref))
        }))
    }
}

impl LuaUserData for LuaChildren {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_meta_method_mut("__call", Self::call);
    }
}

#[self_referencing]
struct LuaDescendants {
    html: Arc<Html>,
    elements_only: bool,

    #[borrows(html)]
    #[covariant]
    iter: Descendants<'this, Node>,
}

impl LuaDescendants {
    fn call(_lua: &Lua, this: &mut Self, _: ()) -> LuaResult<Option<BaseNodeRef>> {
        Ok(this.with_mut(|fields| {
            fields
                .iter
                .next()
                .filter(|node_ref| !*fields.elements_only || node_ref.value().is_element())
                .map(|node_ref| BaseNodeRef::from_node_ref(fields.html.clone(), node_ref))
        }))
    }
}

impl LuaUserData for LuaDescendants {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_meta_method_mut("__call", Self::call);
    }
}

#[self_referencing]
struct LuaNodeRef {
    html: Arc<Html>,

    #[borrows(html)]
    #[covariant]
    node_ref: NodeRef<'this, Node>,
}

impl LuaNodeRef {
    fn from_node_id(html: Arc<Html>, node_id: NodeId) -> Option<Self> {
        Self::try_new(html, |html| match html.tree.get(node_id) {
            Some(node_ref) if matches!(node_ref.value(), Node::Document | Node::Fragment) => {
                Ok(node_ref)
            }
            _ => Err(()),
        })
        .ok()
    }
}

impl IntoBaseNodeRef for LuaNodeRef {
    fn html(&self) -> Arc<Html> {
        self.borrow_html().clone()
    }

    fn as_node_ref(&self) -> NodeRef<'_, Node> {
        *self.borrow_node_ref()
    }
}

impl LuaUserData for LuaNodeRef {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        BaseNodeRef::add_methods(methods);
    }
}

#[self_referencing]
struct LuaDoctypeRef {
    html: Arc<Html>,

    #[borrows(html)]
    #[covariant]
    node_ref: NodeRef<'this, Node>,

    #[borrows(node_ref)]
    doctype: &'this Doctype,
}

impl LuaDoctypeRef {
    fn from_node_id(html: Arc<Html>, node_id: NodeId) -> Option<Self> {
        LuaDoctypeRefTryBuilder {
            html,
            node_ref_builder: |html| html.tree.get(node_id).ok_or(()),
            doctype_builder: |node_ref| node_ref.value().as_doctype().ok_or(()),
        }
        .try_build()
        .ok()
    }

    fn name(_lua: &Lua, this: &Self, _: ()) -> LuaResult<String> {
        Ok(this.borrow_doctype().name().into())
    }

    fn public_id(_lua: &Lua, this: &Self, _: ()) -> LuaResult<String> {
        Ok(this.borrow_doctype().public_id().into())
    }

    fn system_id(_lua: &Lua, this: &Self, _: ()) -> LuaResult<String> {
        Ok(this.borrow_doctype().system_id().into())
    }
}

impl IntoBaseNodeRef for LuaDoctypeRef {
    fn html(&self) -> Arc<Html> {
        self.borrow_html().clone()
    }

    fn as_node_ref(&self) -> NodeRef<'_, Node> {
        *self.borrow_node_ref()
    }
}

impl LuaUserData for LuaDoctypeRef {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("name", Self::name);
        methods.add_method("publicId", Self::public_id);
        methods.add_method("systemId", Self::system_id);

        BaseNodeRef::add_methods(methods);
    }
}

#[self_referencing]
struct LuaCommentRef {
    html: Arc<Html>,

    #[borrows(html)]
    #[covariant]
    node_ref: NodeRef<'this, Node>,

    #[borrows(node_ref)]
    comment: &'this Comment,
}

impl LuaCommentRef {
    fn from_node_id(html: Arc<Html>, node_id: NodeId) -> Option<Self> {
        LuaCommentRefTryBuilder {
            html,
            node_ref_builder: |html| html.tree.get(node_id).ok_or(()),
            comment_builder: |node_ref| node_ref.value().as_comment().ok_or(()),
        }
        .try_build()
        .ok()
    }

    fn to_string(_lua: &Lua, this: &Self, _: ()) -> LuaResult<String> {
        Ok(this.borrow_comment().to_string())
    }

    fn len(_lua: &Lua, this: &Self, _: ()) -> LuaResult<usize> {
        Ok(this.borrow_comment().len())
    }
}

impl IntoBaseNodeRef for LuaCommentRef {
    fn html(&self) -> Arc<Html> {
        self.borrow_html().clone()
    }

    fn as_node_ref(&self) -> NodeRef<'_, Node> {
        *self.borrow_node_ref()
    }
}

impl LuaUserData for LuaCommentRef {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_meta_method("__tostring", Self::to_string);
        methods.add_meta_method("__len", Self::len);

        BaseNodeRef::add_methods(methods);
    }
}

#[self_referencing]
struct LuaTextRef {
    html: Arc<Html>,

    #[borrows(html)]
    #[covariant]
    node_ref: NodeRef<'this, Node>,

    #[borrows(node_ref)]
    text: &'this Text,
}

impl LuaTextRef {
    fn from_node_id(html: Arc<Html>, node_id: NodeId) -> Option<Self> {
        LuaTextRefTryBuilder {
            html,
            node_ref_builder: |html| html.tree.get(node_id).ok_or(()),
            text_builder: |node_ref| node_ref.value().as_text().ok_or(()),
        }
        .try_build()
        .ok()
    }

    fn to_string(_lua: &Lua, this: &Self, _: ()) -> LuaResult<String> {
        Ok(this.borrow_text().to_string())
    }

    fn len(_lua: &Lua, this: &Self, _: ()) -> LuaResult<usize> {
        Ok(this.borrow_text().len())
    }
}

impl IntoBaseNodeRef for LuaTextRef {
    fn html(&self) -> Arc<Html> {
        self.borrow_html().clone()
    }

    fn as_node_ref(&self) -> NodeRef<'_, Node> {
        *self.borrow_node_ref()
    }
}

impl LuaUserData for LuaTextRef {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_meta_method("__tostring", Self::to_string);
        methods.add_meta_method("__len", Self::len);

        BaseNodeRef::add_methods(methods);
    }
}

#[self_referencing]
struct LuaElementRef {
    html: Arc<Html>,

    #[borrows(html)]
    #[covariant]
    element_ref: ElementRef<'this>,
}

impl LuaElementRef {
    fn from_node_id(html: Arc<Html>, node_id: NodeId) -> Option<Self> {
        Self::try_new(html, |html| {
            html.tree.get(node_id).and_then(ElementRef::wrap).ok_or(())
        })
        .ok()
    }

    fn name(_lua: &Lua, this: &Self, _: ()) -> LuaResult<String> {
        Ok(this.borrow_element_ref().value().name().to_string())
    }

    fn html(_lua: &Lua, this: &Self, _: ()) -> LuaResult<String> {
        Ok(this.borrow_element_ref().html())
    }

    fn inner_html(_lua: &Lua, this: &Self, _: ()) -> LuaResult<String> {
        Ok(this.borrow_element_ref().inner_html())
    }

    fn attr(_lua: &Lua, this: &Self, name: Box<str>) -> LuaResult<Option<String>> {
        Ok(this.borrow_element_ref().attr(&name).map(|s| s.to_string()))
    }

    fn attrs(_lua: &Lua, this: &Self, _: ()) -> LuaResult<LuaAttrs> {
        let node_id = this.borrow_element_ref().id();

        Ok(LuaAttrs::new(this.borrow_html().clone(), |html| {
            ElementRef::wrap(html.tree.get(node_id).unwrap())
                .unwrap()
                .value()
                .attrs()
        }))
    }

    fn has_class(
        _lua: &Lua,
        this: &Self,
        (name, case_sensitive): (Box<str>, bool),
    ) -> LuaResult<bool> {
        Ok(this.borrow_element_ref().value().has_class(
            &name,
            if case_sensitive {
                CaseSensitivity::CaseSensitive
            } else {
                CaseSensitivity::AsciiCaseInsensitive
            },
        ))
    }

    fn classes(_lua: &Lua, this: &Self, _: ()) -> LuaResult<LuaClasses> {
        let node_id = this.borrow_element_ref().id();

        Ok(LuaClasses::new(this.borrow_html().clone(), |html| {
            ElementRef::wrap(html.tree.get(node_id).unwrap())
                .unwrap()
                .value()
                .classes()
        }))
    }

    fn text(_lua: &Lua, this: &Self, _: ()) -> LuaResult<LuaElementText> {
        let node_id = this.borrow_element_ref().id();

        Ok(LuaElementText::new(this.borrow_html().clone(), |html| {
            ElementRef::wrap(html.tree.get(node_id).unwrap())
                .unwrap()
                .text()
        }))
    }

    fn child_elements(_lua: &Lua, this: &Self, _: ()) -> LuaResult<LuaChildren> {
        let node_id = this.borrow_element_ref().id();

        Ok(LuaChildrenBuilder {
            html: this.borrow_html().clone(),
            elements_only: true,
            iter_builder: |html| {
                ElementRef::wrap(html.tree.get(node_id).unwrap())
                    .unwrap()
                    .children()
            },
        }
        .build())
    }

    fn descendant_elements(_lua: &Lua, this: &Self, _: ()) -> LuaResult<LuaDescendants> {
        let node_id = this.borrow_element_ref().id();

        Ok(LuaDescendantsBuilder {
            html: this.borrow_html().clone(),
            elements_only: true,
            iter_builder: |html| {
                ElementRef::wrap(html.tree.get(node_id).unwrap())
                    .unwrap()
                    .descendants()
            },
        }
        .build())
    }

    fn select(_lua: &Lua, this: &Self, selector: SelectorWrapper) -> LuaResult<LuaSelect> {
        let node_id = this.borrow_element_ref().id();

        Ok(LuaSelect::new(
            this.borrow_html().clone(),
            selector.0,
            |html, selector| {
                ElementRef::wrap(html.tree.get(node_id).unwrap())
                    .unwrap()
                    .select(selector)
            },
        ))
    }

    fn to_string(_lua: &Lua, this: &Self, _: ()) -> LuaResult<String> {
        let mut text = String::new();

        for s in this.borrow_element_ref().text() {
            text.push_str(s);
        }

        Ok(text)
    }
}

impl IntoBaseNodeRef for LuaElementRef {
    fn html(&self) -> Arc<Html> {
        self.borrow_html().clone()
    }

    fn as_node_ref(&self) -> NodeRef<'_, Node> {
        **self.borrow_element_ref()
    }
}

impl LuaUserData for LuaElementRef {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("name", Self::name);
        methods.add_method("html", Self::html);
        methods.add_method("innerHtml", Self::inner_html);
        methods.add_method("attr", Self::attr);
        methods.add_method("attrs", Self::attrs);
        methods.add_method("hasClass", Self::has_class);
        methods.add_method("classes", Self::classes);
        methods.add_method("text", Self::text);
        methods.add_method("childElements", Self::child_elements);
        methods.add_method("descendantElements", Self::descendant_elements);
        methods.add_method("select", Self::select);
        methods.add_meta_method("__tostring", Self::to_string);

        BaseNodeRef::add_methods(methods);
    }
}

#[self_referencing]
struct LuaAttrs {
    html: Arc<Html>,

    #[borrows(html)]
    #[covariant]
    attrs: Attrs<'this>,
}

impl LuaAttrs {
    fn call(_lua: &Lua, this: &mut Self, _: ()) -> LuaResult<(Option<String>, Option<String>)> {
        Ok(this
            .with_attrs_mut(|attrs| attrs.next())
            .map(|(k, v)| (k.into(), v.into()))
            .unzip())
    }
}

impl LuaUserData for LuaAttrs {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_meta_method_mut("__call", Self::call);
    }
}

#[self_referencing]
struct LuaClasses {
    html: Arc<Html>,

    #[borrows(html)]
    #[covariant]
    classes: Classes<'this>,
}

impl LuaClasses {
    fn call(_lua: &Lua, this: &mut Self, _: ()) -> LuaResult<Option<String>> {
        Ok(this
            .with_classes_mut(|classes| classes.next())
            .map(Into::into))
    }
}

impl LuaUserData for LuaClasses {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_meta_method_mut("__call", Self::call);
    }
}

#[self_referencing]
struct LuaElementText {
    html: Arc<Html>,

    #[borrows(html)]
    #[covariant]
    text: element_ref::Text<'this>,
}

impl LuaElementText {
    fn call(_lua: &Lua, this: &mut Self, _: ()) -> LuaResult<Option<String>> {
        Ok(this.with_text_mut(|text| text.next()).map(Into::into))
    }
}

impl LuaUserData for LuaElementText {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_meta_method_mut("__call", Self::call);
    }
}

#[self_referencing]
struct LuaSelect {
    html: Arc<Html>,
    selector: Arc<Selector>,

    #[borrows(html, selector)]
    #[covariant]
    select: element_ref::Select<'this, 'this>,
}

impl LuaSelect {
    fn call(_lua: &Lua, this: &mut Self, _: ()) -> LuaResult<Option<LuaElementRef>> {
        Ok(this.with_mut(|fields| {
            fields.select.next().map(|element| {
                LuaElementRef::from_node_id(fields.html.clone(), element.id()).unwrap()
            })
        }))
    }
}

impl LuaUserData for LuaSelect {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_meta_method_mut("__call", Self::call);
    }
}

#[self_referencing]
struct LuaProcessingInstructionRef {
    html: Arc<Html>,

    #[borrows(html)]
    #[covariant]
    node_ref: NodeRef<'this, Node>,

    #[borrows(node_ref)]
    pi: &'this ProcessingInstruction,
}

impl LuaProcessingInstructionRef {
    fn from_node_id(html: Arc<Html>, node_id: NodeId) -> Option<Self> {
        LuaProcessingInstructionRefTryBuilder {
            html,
            node_ref_builder: |html| html.tree.get(node_id).ok_or(()),
            pi_builder: |node_ref| node_ref.value().as_processing_instruction().ok_or(()),
        }
        .try_build()
        .ok()
    }

    fn target(_lua: &Lua, this: &Self, _: ()) -> LuaResult<String> {
        Ok(this.borrow_pi().target.clone())
    }

    fn to_string(_lua: &Lua, this: &Self, _: ()) -> LuaResult<String> {
        Ok(this.borrow_pi().to_string())
    }

    fn len(_lua: &Lua, this: &Self, _: ()) -> LuaResult<usize> {
        Ok(this.borrow_pi().len())
    }
}

impl IntoBaseNodeRef for LuaProcessingInstructionRef {
    fn html(&self) -> Arc<Html> {
        self.borrow_html().clone()
    }

    fn as_node_ref(&self) -> NodeRef<'_, Node> {
        *self.borrow_node_ref()
    }
}

impl LuaUserData for LuaProcessingInstructionRef {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("target", Self::target);
        methods.add_meta_method("__tostring", Self::to_string);
        methods.add_meta_method("__len", Self::len);

        BaseNodeRef::add_methods(methods);
    }
}
