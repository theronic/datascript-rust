//! `datascript.pull-api`: pull, as the original runs it: a stack of frames, each walking an entity's datoms beside
//! the pattern's attributes, both in order. No recursion of the machine's own, so a deep pull is as deep as memory.

use crate::cmp::compare;
use crate::coll::CljMap;
use crate::datom::{value_attr, Bound, Datom, Index, E0, EMAX, TX0, TXMAX};
use crate::db::{entid, search, Cursor, Datoms, Db, Searchable};
use crate::error::{Error, Result};
use crate::pull_parser::{parse_attr_name, parse_pattern, PullAttr, PullPattern};
use crate::value::Value;
use std::cmp::Ordering;
use std::sync::{Arc, Mutex};

/// A pattern parsed for a database, and who to tell of what is pulled.
pub struct ParsedOpts {
    pub pattern: Arc<PullPattern>,
    db: Db,
    visitor: Option<Value>,
}

/// `parse-opts`: the pattern, from the last hundred parsed for this schema or parsed now.
pub fn parse_opts(db: &Db, pattern: &Value, visitor: Option<&Value>) -> Result<ParsedOpts> {
    let parsed = db.schema().pull_patterns.get(pattern, || parse_pattern(db, pattern).map(Arc::new))?;
    Ok(ParsedOpts { pattern: parsed, db: db.clone(), visitor: visitor.cloned() })
}

/// `(d/pull db pattern id)`, with an optional `:visitor`.
pub fn pull(db: &Db, pattern: &Value, id: &Value, visitor: Option<&Value>) -> Result<Value> {
    pull_impl(&parse_opts(db, pattern, visitor)?, id)
}

/// `(d/pull-many db pattern ids)`
pub fn pull_many(db: &Db, pattern: &Value, ids: &[Value], visitor: Option<&Value>) -> Result<Vec<Value>> {
    let opts = parse_opts(db, pattern, visitor)?;
    ids.iter().map(|id| pull_impl(&opts, id)).collect()
}

// ---------------------------------------------------------------- the datoms a frame walks

/// A run of datoms read as a lazy sequence is: what has been read is kept, the rest is read when reached. A place
/// in it is a number, so that a frame can hand the place it stopped at to the frame below.
struct Run {
    read: Vec<Datom>,
    rest: Option<(Datoms, Cursor)>,
}

#[derive(Clone)]
struct DatomSeq {
    run: Arc<Mutex<Run>>,
    pos: usize,
}

impl DatomSeq {
    fn of(datoms: Vec<Datom>) -> DatomSeq {
        DatomSeq { run: Arc::new(Mutex::new(Run { read: datoms, rest: None })), pos: 0 }
    }

    fn lazy(datoms: Datoms) -> DatomSeq {
        let cursor = datoms.cursor();
        DatomSeq { run: Arc::new(Mutex::new(Run { read: Vec::new(), rest: Some((datoms, cursor)) })), pos: 0 }
    }

    fn empty() -> DatomSeq {
        DatomSeq::of(Vec::new())
    }

    /// `first-seq`
    fn first(&self) -> Result<Option<Datom>> {
        let mut run = self.run.lock().unwrap_or_else(|e| e.into_inner());
        while run.read.len() <= self.pos {
            let Some((datoms, cursor)) = run.rest.as_mut() else { return Ok(None) };
            let chunk = datoms.next_chunk(cursor, 32)?;
            if chunk.is_empty() {
                run.rest = None;
                return Ok(None);
            }
            run.read.extend(chunk);
        }
        Ok(Some(run.read[self.pos].clone()))
    }

    /// `next-seq`
    fn next(&self) -> DatomSeq {
        DatomSeq { run: self.run.clone(), pos: self.pos + 1 }
    }
}

// ---------------------------------------------------------------- frames

/// The entities already pulled on the way down: a recursive attribute stops at one of them.
#[derive(Clone, Default)]
struct Seen(Option<Arc<SeenNode>>);

struct SeenNode {
    id: i32,
    parent: Option<Arc<SeenNode>>,
}

impl Seen {
    fn contains(&self, id: i32) -> bool {
        let mut node = self.0.as_deref();
        while let Some(n) = node {
            if n.id == id {
                return true;
            }
            node = n.parent.as_deref();
        }
        false
    }

    fn conj(&self, id: i32) -> Seen {
        Seen(Some(Arc::new(SeenNode { id, parent: self.0.clone() })))
    }
}

/// How much further each recursive attribute may be followed.
type Limits = Vec<(Arc<PullAttr>, f64)>;

fn limit_of(limits: &Limits, attr: &Arc<PullAttr>) -> Option<usize> {
    limits.iter().position(|(a, _)| Arc::ptr_eq(a, attr) || **a == **attr)
}

enum Frame {
    /// `ResultFrame`: a value, and where its frame stopped in the datoms
    Result { value: Value, datoms: Option<DatomSeq> },
    /// `MultivalAttrFrame`: the values of an attribute of cardinality many
    MultivalAttr { acc: Vec<Value>, attr: Arc<PullAttr>, datoms: DatomSeq },
    /// `MultivalRefAttrFrame`: the entities an attribute of cardinality many refers to, each pulled
    MultivalRefAttr {
        seen: Seen,
        limits: Limits,
        acc: Vec<Value>,
        pattern: Arc<PullPattern>,
        attr: Arc<PullAttr>,
        datoms: DatomSeq,
    },
    /// `AttrsFrame`: an entity's attributes
    Attrs {
        seen: Seen,
        limits: Limits,
        acc: CljMap,
        pattern: Arc<PullPattern>,
        attr: Option<Arc<PullAttr>>,
        /// The attributes after `attr`, the next of them last
        attrs: Vec<Arc<PullAttr>>,
        datoms: DatomSeq,
        id: i32,
    },
    /// `ReverseAttrsFrame`: the reverse attributes of an entity, after its own
    ReverseAttrs {
        seen: Seen,
        limits: Limits,
        acc: CljMap,
        pattern: Arc<PullPattern>,
        attr: Option<Arc<PullAttr>>,
        attrs: Vec<Arc<PullAttr>>,
        id: i32,
    },
}

/// The attributes of a pattern as a frame reads them: the first, and the rest with the next of them last.
fn attr_seq(attrs: &[Arc<PullAttr>]) -> (Option<Arc<PullAttr>>, Vec<Arc<PullAttr>>) {
    let mut rest: Vec<Arc<PullAttr>> = attrs.iter().rev().cloned().collect();
    let first = rest.pop();
    (first, rest)
}

fn visit(opts: &ParsedOpts, kind: &str, e: Value, a: Value, v: Value) -> Result<()> {
    if let Some(visitor) = &opts.visitor {
        crate::built_ins::call(visitor, &[Value::kw(kind), e, a, v])?;
    }
    Ok(())
}

/// A map's result: `nil` when nothing was pulled.
fn map_or_nil(acc: CljMap) -> Value {
    if acc.is_empty() {
        Value::Nil
    } else {
        Value::map(acc)
    }
}

fn vec_or_nil(acc: Vec<Value>) -> Value {
    if acc.is_empty() {
        Value::Nil
    } else {
        Value::vector(acc)
    }
}

/// The rest of the datoms of an attribute, skipped once its limit is reached.
fn skip_attr(mut datoms: DatomSeq, attr: &PullAttr) -> Result<DatomSeq> {
    loop {
        match datoms.first()? {
            Some(d) if d.a_value() == attr.name => datoms = datoms.next(),
            _ => return Ok(datoms),
        }
    }
}

/// `auto-expanding?`: an attribute pulled as far as it goes: a recursive one, or a component under a wildcard.
fn auto_expanding(attr: &PullAttr) -> bool {
    attr.recursive || (attr.component && attr.pattern.as_ref().is_some_and(|p| p.wildcard))
}

/// `ref-frame`: the frame that pulls the entity a reference leads to.
fn ref_frame(
    opts: &ParsedOpts,
    seen: &Seen,
    limits: &Limits,
    pattern: &Arc<PullPattern>,
    attr: &Arc<PullAttr>,
    id: &Value,
) -> Result<Frame> {
    let id = match id.as_num().map(crate::datom::id_from_num) {
        Some(Ok(id)) => id,
        // a reference to a fraction is to no entity: nothing to pull
        Some(Err(e)) if e.no_such_id => return Ok(Frame::Result { value: Value::Nil, datoms: None }),
        Some(Err(e)) => return Err(e),
        None => return Err(Error::msg(format!("{} is not an entity id", crate::print::pr_str(id)))),
    };
    let own = || attr.pattern.clone().unwrap_or_default();
    if !auto_expanding(attr) {
        return attrs_frame(opts, seen.clone(), limits.clone(), own(), id);
    }
    if seen.contains(id) {
        return Ok(Frame::Result { value: Value::kw_map(&[("db/id", Value::from(id))]), datoms: None });
    }
    let lim = limit_of(limits, attr);
    if let Some(i) = lim {
        if limits[i].1 <= 0.0 {
            return Ok(Frame::Result { value: Value::Nil, datoms: None });
        }
    }
    let mut limits = limits.clone();
    match lim {
        Some(i) => limits[i].1 -= 1.0,
        None => {
            if let Some(n) = attr.recursion_limit.as_num().filter(|_| attr.recursion_limit.truthy()) {
                limits.push((attr.clone(), n - 1.0));
            }
        }
    }
    let next = if attr.recursive { pattern.clone() } else { own() };
    attrs_frame(opts, seen.conj(id), limits, next, id)
}

/// `attrs-frame`: the frame that pulls an entity: its datoms from the first attribute asked for to the last, or all
/// of them under a wildcard.
fn attrs_frame(opts: &ParsedOpts, seen: Seen, limits: Limits, pattern: Arc<PullPattern>, id: i32) -> Result<Frame> {
    let db = &opts.db;
    let datoms = if pattern.wildcard {
        DatomSeq::of(search(db, Some(id), None, None, None).to_vec()?)
    } else if let (Some(first), Some(last)) = (&pattern.first_attr, &pattern.last_attr) {
        if db.is_filtered() {
            // As the original, a filtered database is read from the entity on to the end of the index.
            DatomSeq::lazy(crate::db::seek_datoms(
                db,
                Index::Eavt,
                &Value::from(id),
                &Value::Nil,
                &Value::Nil,
                &Value::Nil,
            )?)
        } else {
            let attr = |a: &PullAttr| {
                value_attr(&a.name)
                    .ok_or_else(|| Error::msg(format!("Cannot compare {}", crate::print::pr_str(&a.name))))
            };
            let from = Bound::new(id, Some(attr(first)?), Value::Nil, TX0);
            let to = Bound::new(id, Some(attr(last)?), Value::Nil, TXMAX);
            DatomSeq::of(db.core().slice_between(Index::Eavt, &from, &to)?)
        }
    } else {
        DatomSeq::empty()
    };
    if pattern.wildcard {
        visit(opts, "db.pull/wildcard", Value::from(id), Value::Nil, Value::Nil)?;
    }
    let (attr, attrs) = attr_seq(&pattern.attrs);
    Ok(Frame::Attrs { seen, limits, acc: CljMap::new(), pattern, attr, attrs, datoms, id })
}

/// `-run`: a frame goes as far as it can by itself, and answers the frames to go on with: its result, or itself
/// with a frame for a reference it met.
fn run(frame: Frame, opts: &ParsedOpts) -> Result<Vec<Frame>> {
    match frame {
        Frame::Result { .. } => Ok(vec![frame]),

        Frame::MultivalAttr { mut acc, attr, mut datoms } => loop {
            match datoms.first()? {
                Some(d) if d.a_value() == attr.name => {
                    if attr.limit().is_some_and(|l| acc.len() as f64 >= l) {
                        // the limit is reached: the rest of the attribute's datoms are passed over
                        let rest = skip_attr(datoms, &attr)?;
                        return Ok(vec![Frame::Result { value: Value::vector(acc), datoms: Some(rest) }]);
                    }
                    acc.push(d.v);
                    datoms = datoms.next();
                }
                _ => return Ok(vec![Frame::Result { value: vec_or_nil(acc), datoms: Some(datoms) }]),
            }
        },

        Frame::MultivalRefAttr { seen, limits, acc, pattern, attr, datoms } => match datoms.first()? {
            Some(d) if d.a_value() == attr.name => {
                if attr.limit().is_some_and(|l| acc.len() as f64 >= l) {
                    let rest = skip_attr(datoms, &attr)?;
                    return Ok(vec![Frame::Result { value: Value::vector(acc), datoms: Some(rest) }]);
                }
                let id = if attr.reverse { Value::from(d.e) } else { d.v.clone() };
                let child = ref_frame(opts, &seen, &limits, &pattern, &attr, &id)?;
                Ok(vec![Frame::MultivalRefAttr { seen, limits, acc, pattern, attr, datoms }, child])
            }
            _ => Ok(vec![Frame::Result { value: vec_or_nil(acc), datoms: Some(datoms) }]),
        },

        Frame::Attrs { seen, limits, mut acc, pattern, mut attr, mut attrs, mut datoms, id } => loop {
            let datom = datoms.first()?;
            let Some(current) = attr.clone() else {
                match datom {
                    // nothing left of either: on to the reverse attributes
                    None => {
                        let (attr, attrs) = attr_seq(&pattern.reverse_attrs);
                        return Ok(vec![Frame::ReverseAttrs { seen, limits, acc, pattern, attr, attrs, id }]);
                    }
                    // a datom no attribute asked for: under a wildcard it is pulled as its attribute would be
                    Some(d) => {
                        if pattern.wildcard {
                            attr = Some(wildcard_attr(&opts.db, &d)?);
                        } else {
                            datoms = datoms.next();
                        }
                        continue;
                    }
                }
            };

            // :db/id
            if matches!(&current.name, Value::Keyword(k) if k.full() == "db/id") {
                acc.assoc(current.as_.clone(), current.transform(Value::from(id))?);
                attr = attrs.pop();
                continue;
            }

            let cmp = match &datom {
                Some(d) => Some(compare(&current.name, &d.a_value())?),
                None => None,
            };
            let attr_ahead = cmp == Some(Ordering::Greater);
            let datom_ahead = datom.is_none() || cmp == Some(Ordering::Less);

            if let (true, Some(d)) = (attr_ahead, &datom) {
                if pattern.wildcard {
                    // the datom's attribute first, and the attribute asked for after it
                    attrs.push(current);
                    attr = Some(wildcard_attr(&opts.db, d)?);
                } else {
                    datoms = datoms.next();
                }
                continue;
            }

            visit(opts, "db.pull/attr", Value::from(id), current.name.clone(), Value::Nil)?;

            if datom_ahead {
                // the entity has no such attribute: its default, or what its xform makes of nothing
                if current.default.is_some() {
                    acc.assoc(current.as_.clone(), current.default.clone());
                } else {
                    let value = current.transform(Value::Nil)?;
                    if value.is_some() {
                        acc.assoc(current.as_.clone(), value);
                    }
                }
                attr = attrs.pop();
                continue;
            }

            let d = datom.expect("a datom of the attribute");
            if current.multival && current.is_ref {
                let child = Frame::MultivalRefAttr {
                    seen: seen.clone(),
                    limits: limits.clone(),
                    acc: Vec::new(),
                    pattern: pattern.clone(),
                    attr: current.clone(),
                    datoms: datoms.clone(),
                };
                return Ok(vec![
                    Frame::Attrs { seen, limits, acc, pattern, attr: Some(current), attrs, datoms, id },
                    child,
                ]);
            }
            if current.multival {
                let child = Frame::MultivalAttr { acc: Vec::new(), attr: current.clone(), datoms: datoms.clone() };
                return Ok(vec![
                    Frame::Attrs { seen, limits, acc, pattern, attr: Some(current), attrs, datoms, id },
                    child,
                ]);
            }
            if current.is_ref {
                let child = ref_frame(opts, &seen, &limits, &pattern, &current, &d.v)?;
                return Ok(vec![
                    Frame::Attrs { seen, limits, acc, pattern, attr: Some(current), attrs, datoms, id },
                    child,
                ]);
            }
            acc.assoc(current.as_.clone(), current.transform(d.v)?);
            attr = attrs.pop();
            datoms = datoms.next();
        },

        Frame::ReverseAttrs { seen, limits, mut acc, pattern, mut attr, mut attrs, id } => loop {
            let Some(current) = attr.clone() else {
                return Ok(vec![Frame::Result { value: map_or_nil(acc), datoms: None }]);
            };
            let db = &opts.db;
            let name = value_attr(&current.name)
                .ok_or_else(|| Error::msg(format!("Cannot compare {}", crate::print::pr_str(&current.name))))?;
            let target = Value::from(id);
            let datoms = if db.is_filtered() {
                search(db, None, Some(&name), Some(&target), None).to_vec()?
            } else {
                let from = Bound::new(E0, Some(name.clone()), target.clone(), TX0);
                let to = Bound::new(EMAX, Some(name), target.clone(), TXMAX);
                db.core().slice_between(Index::Avet, &from, &to)?
            };
            visit(opts, "db.pull/reverse", Value::Nil, current.name.clone(), target)?;
            if datoms.is_empty() {
                if current.default.is_some() {
                    acc.assoc(current.as_.clone(), current.default.clone());
                }
                attr = attrs.pop();
                continue;
            }
            let child = if current.component {
                ref_frame(opts, &seen, &limits, &pattern, &current, &Value::from(datoms[0].e))?
            } else {
                Frame::MultivalRefAttr {
                    seen: seen.clone(),
                    limits: limits.clone(),
                    acc: Vec::new(),
                    pattern: pattern.clone(),
                    attr: current.clone(),
                    datoms: DatomSeq::of(datoms),
                }
            };
            return Ok(vec![Frame::ReverseAttrs { seen, limits, acc, pattern, attr: Some(current), attrs, id }, child]);
        },
    }
}

/// The attribute a wildcard pulls a datom as: from the last hundred met under this schema, or parsed now.
///
/// An attribute named as a reverse reference is, `:_ref`, which `[:db/add e :_ref v]` stores as it is written, would
/// be parsed as the reference backwards, which the datom is not of: the original then asks for it again without
/// end. Here it is pulled as the plain attribute it is.
fn wildcard_attr(db: &Db, d: &Datom) -> Result<Arc<PullAttr>> {
    let name = d.a_value();
    db.schema().pull_attrs.get(&name, || {
        let attr = match parse_attr_name(db, &name) {
            Ok(attr) if attr.name == name => attr,
            _ => {
                let p = crate::db::props_of(db, &name);
                PullAttr {
                    as_: name.clone(),
                    default: Value::Nil,
                    limit: if p.many { Value::from(1000) } else { Value::Nil },
                    name: name.clone(),
                    pattern: None,
                    recursion_limit: Value::Nil,
                    recursive: false,
                    reverse: false,
                    xform: None,
                    multival: p.many,
                    is_ref: false,
                    component: false,
                }
            }
        };
        Ok(Arc::new(attr))
    })
}

/// `-merge`: a frame takes the result of the frame it started.
fn merge(frame: Frame, value: Value, result_datoms: Option<DatomSeq>) -> Result<Frame> {
    match frame {
        Frame::MultivalRefAttr { seen, limits, mut acc, pattern, attr, datoms } => {
            if value.is_some() {
                acc.push(value);
            }
            Ok(Frame::MultivalRefAttr { seen, limits, acc, pattern, attr, datoms: datoms.next() })
        }
        Frame::Attrs { seen, limits, mut acc, pattern, attr, mut attrs, datoms, id } => {
            let current = attr.expect("the attribute the result is of");
            let value = current.transform(value)?;
            if value.is_some() {
                acc.assoc(current.as_.clone(), value);
            }
            let attr = attrs.pop();
            let datoms = result_datoms.unwrap_or_else(|| datoms.next());
            Ok(Frame::Attrs { seen, limits, acc, pattern, attr, attrs, datoms, id })
        }
        Frame::ReverseAttrs { seen, limits, mut acc, pattern, attr, mut attrs, id } => {
            let current = attr.expect("the attribute the result is of");
            let value = current.transform(value)?;
            if value.is_some() {
                acc.assoc(current.as_.clone(), value);
            }
            let attr = attrs.pop();
            Ok(Frame::ReverseAttrs { seen, limits, acc, pattern, attr, attrs, id })
        }
        Frame::Result { .. } | Frame::MultivalAttr { .. } => {
            Err(Error::msg("No protocol method IFrame.-merge defined"))
        }
    }
}

/// `pull-impl`: the entity pulled through the pattern; `nil` when there is no such entity, or nothing of it to pull.
pub fn pull_impl(opts: &ParsedOpts, id: &Value) -> Result<Value> {
    let Some(eid) = entid(&opts.db, id).or_else(Error::or_nothing)? else { return Ok(Value::Nil) };
    let mut stack = vec![attrs_frame(opts, Seen::default(), Vec::new(), opts.pattern.clone(), eid)?];
    loop {
        let last = stack.pop().expect("a frame");
        match last {
            Frame::Result { value, datoms } => match stack.pop() {
                None => return Ok(value),
                Some(penultimate) => stack.push(merge(penultimate, value, datoms)?),
            },
            other => stack.extend(run(other, opts)?),
        }
    }
}
