//! `datascript.pull-parser`: a pull pattern, parsed against a database's schema.
//!
//! pattern             = [(attr-spec | map-spec | '* | "*")+]
//! attr-spec           = attr-name | attr-expr | legacy-limit-expr | legacy-default-expr
//! attr-name           = an edn keyword that names an attr
//! attr-expr           = [attr-name attr-option+]
//! map-spec            = {attr-spec (pattern | recursion-limit)}
//! attr-option         = :as any-value | :limit multival-limit | :default any-value | :xform symbol
//! recursion-limit     = positive-number | '...
//! multival-limit      = positive-number | nil
//! legacy-limit-expr   = [("limit" | 'limit) attr-spec multival-limit]
//! legacy-default-expr = [("default" | 'default) attr-spec any-value]

use crate::built_ins::query_fn;
use crate::cmp::compare;
use crate::db::{props_of, Db};
use crate::error::{Error, Result};
use crate::named::Keyword;
use crate::print::pr_str;
use crate::record::record;
use crate::transact::{is_reverse_ref, reverse_ref};
use crate::value::Value;
use crate::{clj, raise};
use std::sync::{Arc, OnceLock};

/// `PullAttr`
#[derive(Clone)]
pub struct PullAttr {
    /// The key the value goes under: the attribute as written, or `:as`
    pub as_: Value,
    pub default: Value,
    /// How many values of an attribute of cardinality many are pulled; `nil` for all
    pub limit: Value,
    /// The attribute, forwards
    pub name: Value,
    pub pattern: Option<Arc<PullPattern>>,
    pub recursion_limit: Value,
    pub recursive: bool,
    pub reverse: bool,
    /// `:xform`; `None` is `identity`
    pub xform: Option<Value>,
    pub multival: bool,
    pub is_ref: bool,
    pub component: bool,
}

impl PullAttr {
    /// `((.-xform attr) value)`
    pub fn transform(&self, value: Value) -> Result<Value> {
        match &self.xform {
            None => Ok(value),
            Some(f) => crate::built_ins::call(f, &[value]),
        }
    }

    /// The limit, when there is one
    pub fn limit(&self) -> Option<f64> {
        if self.limit.truthy() {
            self.limit.as_num()
        } else {
            None
        }
    }
}

/// Two attributes are one in a map of recursion limits when their records are equal.
impl PartialEq for PullAttr {
    fn eq(&self, o: &PullAttr) -> bool {
        self.as_ == o.as_
            && self.default == o.default
            && self.limit == o.limit
            && self.name == o.name
            && match (&self.pattern, &o.pattern) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
                _ => false,
            }
            && self.recursion_limit == o.recursion_limit
            && self.recursive == o.recursive
            && self.reverse == o.reverse
            && self.xform == o.xform
            && self.multival == o.multival
            && self.is_ref == o.is_ref
            && self.component == o.component
    }
}

/// `PullPattern`
#[derive(Clone, Default, PartialEq)]
pub struct PullPattern {
    /// The attributes, in the order of their names
    pub attrs: Vec<Arc<PullAttr>>,
    /// The first and last of them that are not `:db/id`: where the entity's datoms are read from and to
    pub first_attr: Option<Arc<PullAttr>>,
    pub last_attr: Option<Arc<PullAttr>>,
    pub reverse_attrs: Vec<Arc<PullAttr>>,
    pub wildcard: bool,
}

/// `default-db-id-attr`
fn default_db_id_attr() -> Arc<PullAttr> {
    static ATTR: OnceLock<Arc<PullAttr>> = OnceLock::new();
    ATTR.get_or_init(|| {
        Arc::new(PullAttr {
            as_: Value::kw("db/id"),
            default: Value::Nil,
            limit: Value::Nil,
            name: Value::kw("db/id"),
            pattern: None,
            recursion_limit: Value::Nil,
            recursive: false,
            reverse: false,
            xform: None,
            multival: false,
            is_ref: false,
            component: false,
        })
    })
    .clone()
}

/// `default-pattern-ref`: a reference pulls its `:db/id`
fn default_pattern_ref() -> Arc<PullPattern> {
    static P: OnceLock<Arc<PullPattern>> = OnceLock::new();
    P.get_or_init(|| Arc::new(PullPattern { attrs: vec![default_db_id_attr()], ..Default::default() })).clone()
}

/// `default-pattern-component`: a component pulls everything
fn default_pattern_component() -> Arc<PullPattern> {
    static P: OnceLock<Arc<PullPattern>> = OnceLock::new();
    P.get_or_init(|| Arc::new(PullPattern { attrs: vec![default_db_id_attr()], wildcard: true, ..Default::default() }))
        .clone()
}

/// `check`
fn check(cond: bool, expected: &str, fragment: &Value) -> Result<()> {
    if cond {
        Ok(())
    } else {
        Err(Error::new(
            format!("Expected {expected}, got: {}", pr_str(fragment)),
            Value::kw_map(&[("error", Value::kw("parser/pull")), ("fragment", fragment.clone())]),
        ))
    }
}

/// `parse-attr-name`
pub fn parse_attr_name(db: &Db, attr_spec: &Value) -> Result<PullAttr> {
    let reverse = is_reverse_ref(attr_spec)?;
    let name = if reverse { reverse_ref(attr_spec)? } else { attr_spec.clone() };
    let p = props_of(db, &name);
    let (is_ref, component, multival) = (p.is_ref, p.component, p.many);
    if reverse {
        check(is_ref, "reverse attribute having :db.type/ref", attr_spec)?;
    }
    Ok(PullAttr {
        as_: attr_spec.clone(),
        default: Value::Nil,
        limit: if multival { Value::from(1000) } else { Value::Nil },
        name,
        pattern: if !is_ref {
            None
        } else if reverse {
            Some(default_pattern_ref())
        } else if component {
            Some(default_pattern_component())
        } else {
            Some(default_pattern_ref())
        },
        recursion_limit: Value::Nil,
        recursive: false,
        reverse,
        xform: None,
        multival,
        is_ref,
        component,
    })
}

fn check_limit(db: &Db, name: &Value, limit: &Value) -> Result<()> {
    check(limit.is_nil() || limit.as_num().is_some_and(|n| n > 0.0), "(positive-number | nil)", limit)?;
    check(props_of(db, name).many, "limit attribute having :db.cardinality/many", name)
}

/// `resolve-xform`: a function, or the name of a built-in one
fn resolve_xform(f: &Value) -> Result<Value> {
    if f.is_fn() {
        return Ok(f.clone());
    }
    if let Some(found) = f.as_symbol().and_then(query_fn) {
        return Ok(found);
    }
    raise!("Can't resolve symbol ", f; {"error" => Value::kw("parser/pull"), "fragment" => f.clone()})
}

/// `parse-attr-expr`: `[attr-name :as … :limit … :default … :xform …]`
fn parse_attr_expr(db: &Db, attr_spec: &Value, items: &[Value]) -> Result<Option<PullAttr>> {
    let Some(mut pull_attr) = parse_attr_spec(db, items.first().unwrap_or(&Value::Nil))? else { return Ok(None) };
    let opts = items.get(1..).unwrap_or(&[]);
    check(opts.len() % 2 == 0, "even number of opts", attr_spec)?;
    for kv in opts.chunks(2) {
        let (key, value) = (&kv[0], &kv[1]);
        match key.as_keyword().map(Keyword::full) {
            Some("as") => pull_attr.as_ = value.clone(),
            Some("limit") => {
                check_limit(db, &pull_attr.name, value)?;
                pull_attr.limit = value.clone();
            }
            Some("default") => pull_attr.default = value.clone(),
            Some("xform") => pull_attr.xform = Some(resolve_xform(value)?),
            _ => check(false, "one of :as, :limit, :default, :xform", attr_spec)?,
        }
    }
    Ok(Some(pull_attr))
}

fn is_legacy(head: Option<&Value>, name: &str) -> bool {
    match head {
        Some(Value::Symbol(s)) => s.full() == name,
        Some(Value::Str(s)) => &**s == name,
        _ => false,
    }
}

/// `parse-attr-spec`: an attribute's name, an expression of it, or neither
fn parse_attr_spec(db: &Db, attr_spec: &Value) -> Result<Option<PullAttr>> {
    match attr_spec {
        Value::Keyword(_) => parse_attr_name(db, attr_spec).map(Some),
        Value::Str(s) if !matches!(&**s, "default" | "limit") => parse_attr_name(db, attr_spec).map(Some),
        Value::Vector(items) | Value::List(items) => {
            if let Some(attr) = parse_attr_expr(db, attr_spec, items)? {
                return Ok(Some(attr));
            }
            // ['limit attr-name (positive-number | nil)]
            if is_legacy(items.first(), "limit") {
                check(items.len() == 3, "['limit attr-name (positive-number | nil)]", attr_spec)?;
                let parsed = parse_attr_spec(db, &items[1])?;
                let name = parsed.as_ref().map_or(Value::Nil, |a| a.name.clone());
                check_limit(db, &name, &items[2])?;
                let mut attr = parsed.ok_or_else(|| Error::msg("No attribute to limit"))?;
                attr.limit = items[2].clone();
                return Ok(Some(attr));
            }
            // ['default attr-name any-value]
            if is_legacy(items.first(), "default") {
                check(items.len() == 3, "['default attr-name any-value]", attr_spec)?;
                let mut attr = parse_attr_spec(db, &items[1])?.ok_or_else(|| Error::msg("No attribute to default"))?;
                attr.default = items[2].clone();
                return Ok(Some(attr));
            }
            check(
                false,
                "[attr-name attr-option+] | ['limit attr-name (positive-num | nil)] | ['default attr-name any-val]",
                attr_spec,
            )?;
            Ok(None)
        }
        _ => Ok(None),
    }
}

/// `parse-map-spec`: `{attr-spec pattern}`, a reference and what to pull through it
fn parse_map_spec(db: &Db, attr_spec: &Value, pattern: &Value) -> Result<PullAttr> {
    let pull_attr = parse_attr_spec(db, attr_spec)?;
    check(pull_attr.is_some(), "attr-name | attr-expr", attr_spec)?;
    let mut pull_attr = pull_attr.expect("checked");
    check(props_of(db, &pull_attr.name).is_ref, "attribute having :db.type/ref", attr_spec)?;
    let dots = pattern.is_sym("...") || matches!(pattern, Value::Str(s) if &**s == "...");
    if dots {
        pull_attr.pattern = None;
        pull_attr.recursive = true;
        pull_attr.recursion_limit = Value::Nil;
    } else if let Value::Num(n) = pattern {
        let spec = Value::map([(attr_spec.clone(), pattern.clone())].into_iter().collect());
        check(*n > 0.0, "(positive-num | ...)", &spec)?;
        pull_attr.pattern = None;
        pull_attr.recursive = true;
        pull_attr.recursion_limit = pattern.clone();
    } else {
        pull_attr.pattern = Some(Arc::new(parse_pattern(db, pattern)?));
    }
    Ok(pull_attr)
}

/// `conj-attr`: an attribute into the pattern, in place of one with the same `:as`
fn conj_attr(attrs: &mut Vec<Arc<PullAttr>>, reverse_attrs: &mut Vec<Arc<PullAttr>>, pull_attr: PullAttr) {
    let list = if pull_attr.reverse { reverse_attrs } else { attrs };
    match list.iter().position(|a| a.as_ == pull_attr.as_) {
        Some(idx) => list[idx] = Arc::new(pull_attr),
        None => list.push(Arc::new(pull_attr)),
    }
}

fn is_db_id(attr: &PullAttr) -> bool {
    match &attr.name {
        Value::Keyword(k) => k.full() == "db/id",
        Value::Str(s) => &**s == ":db/id",
        _ => false,
    }
}

/// The key attributes are ordered by: the name as a keyword
fn key_of(attr: &PullAttr) -> Value {
    match &attr.name {
        Value::Str(s) => Value::Keyword(Keyword::parse(s.strip_prefix(':').unwrap_or(s))),
        other => other.clone(),
    }
}

/// `parse-pattern`
pub fn parse_pattern(db: &Db, pattern: &Value) -> Result<PullPattern> {
    check(pattern.is_sequential(), "pattern to be sequential?", pattern)?;
    let mut attrs: Vec<Arc<PullAttr>> = Vec::new();
    let mut reverse_attrs: Vec<Arc<PullAttr>> = Vec::new();
    let mut wildcard = false;
    for attr_spec in pattern.as_seq().unwrap_or(&[]) {
        let star = attr_spec.is_sym("*")
            || matches!(attr_spec, Value::Str(s) if &**s == "*")
            || matches!(attr_spec, Value::Keyword(k) if k.full() == "*");
        if star {
            wildcard = true;
        } else if let Value::Map(m) = attr_spec {
            for (spec, sub) in m.iter() {
                let attr = parse_map_spec(db, spec, sub)?;
                conj_attr(&mut attrs, &mut reverse_attrs, attr);
            }
        } else {
            match parse_attr_spec(db, attr_spec)? {
                Some(attr) => conj_attr(&mut attrs, &mut reverse_attrs, attr),
                None => check(false, "attr-name | attr-expr | map-spec | *", attr_spec)?,
            }
        }
    }
    if wildcard && !attrs.iter().any(|a| is_db_id(a)) {
        attrs.push(default_db_id_attr());
    }
    let sort = |list: &mut Vec<Arc<PullAttr>>| -> Result<()> {
        let mut keyed: Vec<Value> = (0..list.len()).map(Value::from).collect();
        let keys: Vec<Value> = list.iter().map(|a| key_of(a)).collect();
        clj::sort_by(&mut keyed, |a, b| {
            compare(&keys[a.as_num().unwrap_or(0.0) as usize], &keys[b.as_num().unwrap_or(0.0) as usize])
        })?;
        *list = keyed.iter().map(|i| list[i.as_num().unwrap_or(0.0) as usize].clone()).collect();
        Ok(())
    };
    sort(&mut attrs)?;
    sort(&mut reverse_attrs)?;
    let first_attr = attrs.iter().find(|a| !is_db_id(a)).cloned();
    let last_attr = attrs.iter().rev().find(|a| !is_db_id(a)).cloned();
    Ok(PullPattern { attrs, first_attr, last_attr, reverse_attrs, wildcard })
}

/// The parsed pattern as ClojureScript prints `(into {} pattern)`: a map of the pattern's fields, the attributes
/// and patterns within as their records, a function as `:fn`.
pub fn pattern_to_value(p: &PullPattern) -> Value {
    const NS: &str = "datascript.pull-parser";
    fn flag(b: bool) -> Value {
        if b {
            Value::Bool(true)
        } else {
            Value::Nil
        }
    }
    fn attr(a: &PullAttr) -> Value {
        record(
            NS,
            "PullAttr",
            vec![
                ("as", a.as_.clone()),
                ("default", a.default.clone()),
                ("limit", a.limit.clone()),
                ("name", a.name.clone()),
                ("pattern", a.pattern.as_ref().map_or(Value::Nil, |p| record(NS, "PullPattern", fields(p)))),
                ("recursion-limit", a.recursion_limit.clone()),
                ("recursive?", flag(a.recursive)),
                ("reverse?", flag(a.reverse)),
                ("xform", Value::kw("fn")),
                ("multival?", flag(a.multival)),
                ("ref?", flag(a.is_ref)),
                ("component?", flag(a.component)),
            ],
        )
    }
    fn list(attrs: &[Arc<PullAttr>]) -> Value {
        if attrs.is_empty() {
            Value::Nil
        } else {
            Value::list(attrs.iter().map(|a| attr(a)).collect())
        }
    }
    fn fields(p: &PullPattern) -> Vec<(&'static str, Value)> {
        vec![
            ("attrs", list(&p.attrs)),
            ("first-attr", p.first_attr.as_ref().map_or(Value::Nil, |a| attr(a))),
            ("last-attr", p.last_attr.as_ref().map_or(Value::Nil, |a| attr(a))),
            ("reverse-attrs", list(&p.reverse_attrs)),
            ("wildcard?", flag(p.wildcard)),
        ]
    }
    Value::map(fields(p).into_iter().map(|(k, v)| (Value::kw(k), v)).collect())
}
