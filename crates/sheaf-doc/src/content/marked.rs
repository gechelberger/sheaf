//! Marked content: the ranges `BMC` and `BDC` open and `EMC` ends, found
//! across a page's streams, each with its tag and property list. A named
//! property list is looked up in the resources' `/Properties`.
//!
//! Marked content must balance: an `EMC` that ends nothing, or a range
//! still open at the end, is a [`LexError`], since a range to remove can't
//! be told from the content around it.

use std::collections::HashMap;

use lopdf::{Dictionary, Object};

use super::lex::{self, LexError, LexErrorKind, ResCx, Step, Tok};
use super::page::{Concat, ReadObjects, SpanAcross, direct};
use crate::source::Limits;

/// What named property lists name: a page's or a form's `/Resources
/// /Properties`.
#[derive(Debug, Clone, Default)]
pub struct PropertiesDict {
    lists: HashMap<Vec<u8>, Dictionary>,
}

impl PropertiesDict {
    /// The property lists of `resources`, as written.
    pub fn read(objects: &impl ReadObjects, resources: Option<&Object>) -> PropertiesDict {
        let lists = resources
            .and_then(|r| direct(objects, r))
            .and_then(|r| r.as_dict().ok())
            .and_then(|r| direct(objects, r.get(b"Properties").ok()?))
            .and_then(|p| p.as_dict().ok());
        PropertiesDict {
            lists: lists
                .into_iter()
                .flat_map(|lists| lists.iter())
                .filter_map(|(name, list)| {
                    let list = direct(objects, list)?.as_dict().ok()?;
                    Some((name.clone(), list.clone()))
                })
                .collect(),
        }
    }

    /// The list named `name`.
    pub fn get(&self, name: &[u8]) -> Option<&Dictionary> {
        self.lists.get(name)
    }
}

/// A marked range's property list.
#[derive(Debug, Clone, PartialEq)]
pub enum Props {
    /// Written in the content: `/Tag << … >> BDC`.
    Inline(Dictionary),
    /// Named there, `/Tag /Name BDC`: the name, and the list the
    /// resources have under it, if they have one.
    Named(Vec<u8>, Option<Dictionary>),
}

impl Props {
    /// The list itself, wherever it's written.
    pub fn dict(&self) -> Option<&Dictionary> {
        match self {
            Props::Inline(dict) => Some(dict),
            Props::Named(_, dict) => dict.as_ref(),
        }
    }
}

/// A marked range: from its tag, before `BMC` or `BDC`, through its `EMC`.
#[derive(Debug, Clone, PartialEq)]
pub struct Marked {
    pub tag: Vec<u8>,
    /// None for `BMC`, or a `BDC` whose list is neither a dictionary nor
    /// a name.
    pub props: Option<Props>,
    pub span: SpanAcross,
    /// The ranges it's inside: 0 at the top.
    pub depth: u16,
}

/// `content`'s marked ranges, in the order they open, lexed with `res`.
/// Named lists are looked up in `props`.
pub fn marked(
    content: &Concat,
    props: &PropertiesDict,
    res: &ResCx,
    lim: &Limits,
) -> Result<Vec<Marked>, LexError> {
    let mut nest = Nest::new(props, lim);
    lex::steps(content.bytes(), res, lim, |step| {
        nest.step(content, &step).map(drop)
    })?;
    nest.done()
}

/// Marked content as content is stepped through: the ranges open, and
/// every range found, in the order they open.
pub(super) struct Nest<'p> {
    props: &'p PropertiesDict,
    depth: usize,
    /// Each range open: where it is in `found`, and where it starts.
    open: Vec<(usize, usize)>,
    found: Vec<Marked>,
}

/// What a step did to marked content: the range it opened or ended, by
/// where it is in [`Nest::found`].
pub(super) enum Event {
    Opened(usize),
    Ended(usize),
}

impl<'p> Nest<'p> {
    pub(super) fn new(props: &'p PropertiesDict, lim: &Limits) -> Nest<'p> {
        Nest {
            props,
            depth: lim.content.depth,
            open: Vec::new(),
            found: Vec::new(),
        }
    }

    pub(super) fn step(
        &mut self,
        content: &Concat,
        step: &Step,
    ) -> Result<Option<Event>, LexError> {
        let Tok::Op(op @ (b"BMC" | b"BDC" | b"EMC")) = step.tok else {
            return Ok(None);
        };
        if op == b"EMC" {
            let (index, start) = self.open.pop().ok_or(LexError {
                at: step.at.start,
                kind: LexErrorKind::Emc,
            })?;
            self.found[index].span = content.span(start..step.at.end);
            return Ok(Some(Event::Ended(index)));
        }
        if self.open.len() >= self.depth {
            return Err(LexError {
                at: step.at.start,
                kind: LexErrorKind::Depth,
            });
        }
        let (tag, props) = match op {
            b"BDC" => (step.name(1), step.arg(0).and_then(|p| self.props(p))),
            _ => (step.name(0), None),
        };
        let start = step.range().start;
        let index = self.found.len();
        self.found.push(Marked {
            tag: tag.map(|t| t.into_owned()).unwrap_or_default(),
            props,
            span: content.span(start..start),
            depth: u16::try_from(self.open.len()).unwrap_or(u16::MAX),
        });
        self.open.push((index, start));
        Ok(Some(Event::Opened(index)))
    }

    /// A `BDC`'s list, `operand` as written.
    fn props(&self, operand: &[u8]) -> Option<Props> {
        if let Some(name) = lex::name(operand) {
            let list = self.props.get(&name).cloned();
            return Some(Props::Named(name.into_owned(), list));
        }
        match lex::object(operand, self.depth)? {
            Object::Dictionary(dict) => Some(Props::Inline(dict)),
            _ => None,
        }
    }

    /// The range at `index` in [`Nest::found`].
    pub(super) fn get(&self, index: usize) -> &Marked {
        &self.found[index]
    }

    /// Every range found, once content has ended; an error when one is
    /// still open.
    pub(super) fn done(self) -> Result<Vec<Marked>, LexError> {
        match self.open.first() {
            Some(&(_, start)) => Err(LexError {
                at: start,
                kind: LexErrorKind::Marked,
            }),
            None => Ok(self.found),
        }
    }
}
