//! The schema, and what DataScript derives from it (`rschema`): which attributes are references, of cardinality
//! many, unique, indexed, components, tuples.
//!
//! The derived sets are built as ClojureScript builds them, since their order shows: retracting an entity retracts
//! the references to it attribute by attribute, in the order of the set of reference attributes.

use crate::coll::{CljMap, CljSet};
use crate::datom::value_attr;
use crate::error::{Error, Result};
use crate::lru::Cache;
use crate::named::{Attr, Keyword};
use crate::print::pr_str;
use crate::value::Value;
use crate::{message, raise};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// The keywords of a schema, interned once.
pub(crate) struct Kw {
    pub db_id: Keyword,
    pub db_ident: Keyword,
    pub db_fn: Keyword,
    pub db_unique: Keyword,
    pub db_unique_identity: Keyword,
    pub db_unique_value: Keyword,
    pub db_index: Keyword,
    pub db_cardinality: Keyword,
    pub db_cardinality_one: Keyword,
    pub db_cardinality_many: Keyword,
    pub db_value_type: Keyword,
    pub db_type_ref: Keyword,
    pub db_type_tuple: Keyword,
    pub db_is_component: Keyword,
    pub db_tuple_attrs: Keyword,
    pub db_tuple_types: Keyword,
    pub db_tuple_type: Keyword,
    pub db_attr_tuples: Keyword,
    pub db_add: Keyword,
    pub db_retract: Keyword,
    pub db_cas: Keyword,
    pub db_fn_cas: Keyword,
    pub db_fn_call: Keyword,
    pub db_fn_retract_attribute: Keyword,
    pub db_fn_retract_entity: Keyword,
    pub db_retract_entity: Keyword,
    pub db_current_tx: Keyword,
}

pub(crate) fn kw() -> &'static Kw {
    static KW: OnceLock<Kw> = OnceLock::new();
    KW.get_or_init(|| {
        let k = Keyword::parse;
        Kw {
            db_id: k("db/id"),
            db_ident: k("db/ident"),
            db_fn: k("db/fn"),
            db_unique: k("db/unique"),
            db_unique_identity: k("db.unique/identity"),
            db_unique_value: k("db.unique/value"),
            db_index: k("db/index"),
            db_cardinality: k("db/cardinality"),
            db_cardinality_one: k("db.cardinality/one"),
            db_cardinality_many: k("db.cardinality/many"),
            db_value_type: k("db/valueType"),
            db_type_ref: k("db.type/ref"),
            db_type_tuple: k("db.type/tuple"),
            db_is_component: k("db/isComponent"),
            db_tuple_attrs: k("db/tupleAttrs"),
            db_tuple_types: k("db/tupleTypes"),
            db_tuple_type: k("db/tupleType"),
            db_attr_tuples: k("db/attrTuples"),
            db_add: k("db/add"),
            db_retract: k("db/retract"),
            db_cas: k("db/cas"),
            db_fn_cas: k("db.fn/cas"),
            db_fn_call: k("db.fn/call"),
            db_fn_retract_attribute: k("db.fn/retractAttribute"),
            db_fn_retract_entity: k("db.fn/retractEntity"),
            db_retract_entity: k("db/retractEntity"),
            db_current_tx: k("db/current-tx"),
        }
    })
}

/// What the schema says of one attribute.
#[derive(Default, Clone)]
pub struct AttrProps {
    pub many: bool,
    pub is_ref: bool,
    pub unique: bool,
    pub unique_identity: bool,
    pub unique_value: bool,
    pub index: bool,
    pub component: bool,
    /// `:db.type/tuple`: it has `:db/tupleAttrs`, `:db/tupleTypes` or `:db/tupleType`
    pub tuple: bool,
    /// A composite tuple, made of other attributes' values
    pub composite: bool,
    /// For an attribute a composite tuple is made of: each such tuple and this attribute's place in it, in the
    /// order of ClojureScript's map of them
    pub attr_tuples: Vec<(Attr, usize)>,
    /// The attribute's own `:db/tupleAttrs`, `:db/tupleTypes` and `:db/tupleType`, where its schema has the key
    pub tuple_attrs: Option<Value>,
    pub tuple_types: Option<Value>,
    pub tuple_type: Option<Value>,
}

pub struct Schema {
    /// The schema as it was given: a map, or `nil`
    pub(crate) value: Value,
    /// `:rschema`, as ClojureScript's DataScript holds it
    pub(crate) rschema: Value,
    props: HashMap<Attr, AttrProps>,
    /// The reference attributes, in the order of their set
    pub(crate) ref_attrs: Vec<Attr>,
    /// Whether any attribute is part of a composite tuple
    pub(crate) has_tuples: bool,
    /// The last pull patterns parsed against this schema, and the attributes a wildcard met
    pub(crate) pull_patterns: Cache<Value, Arc<crate::pull_parser::PullPattern>>,
    pub(crate) pull_attrs: Cache<Value, Arc<crate::pull_parser::PullAttr>>,
    /// A number that is this schema's alone, for a host that keeps what it has read of a schema
    uid: u32,
}

static NO_PROPS: OnceLock<AttrProps> = OnceLock::new();

impl Schema {
    /// A schema validated, as `empty-db` and `init-db` take it.
    pub fn new(schema: Value) -> Result<Schema> {
        if !(schema.is_nil() || schema.is_map()) {
            return Err(Error::msg("Assert failed: (or (nil? schema) (map? schema))"));
        }
        validate_schema(&schema)?;
        Ok(Schema::unchecked(schema))
    }

    /// A schema as it is, as `with-schema` takes it.
    pub fn unchecked(schema: Value) -> Schema {
        let (rschema, props, ref_attrs) = build_rschema(&schema);
        let has_tuples = props.values().any(|p| !p.attr_tuples.is_empty());
        Schema {
            value: schema,
            rschema,
            props,
            ref_attrs,
            has_tuples,
            pull_patterns: Cache::new(100),
            pull_attrs: Cache::new(100),
            uid: {
                static UID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
                UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            },
        }
    }

    /// The schema's own number: two database values of one number have one schema.
    pub fn uid(&self) -> u32 {
        self.uid
    }

    #[inline]
    pub fn props(&self, a: &Attr) -> &AttrProps {
        self.props.get(a).unwrap_or_else(|| NO_PROPS.get_or_init(AttrProps::default))
    }

    /// The properties of a value used as an attribute: those of a keyword or string, none of anything else.
    #[inline]
    pub fn props_of(&self, a: &Value) -> &AttrProps {
        match a {
            Value::Keyword(k) => self.props.get(&Attr::keyword(k)),
            Value::Str(s) => self.props.get(&Attr::string(s)),
            _ => None,
        }
        .unwrap_or_else(|| NO_PROPS.get_or_init(AttrProps::default))
    }
}

/// `attr->properties`: what one entry of an attribute's schema says of it.
fn attr_properties(k: &Value, v: &Value) -> &'static [&'static str] {
    let kw = kw();
    if k.is_kw(&kw.db_tuple_attrs) {
        &["db.type/tuple", "db/index", "db/tupleAttrs"]
    } else if k.is_kw(&kw.db_tuple_types) {
        &["db.type/tuple", "db/tupleTypes"]
    } else if k.is_kw(&kw.db_tuple_type) {
        &["db.type/tuple", "db/tupleType"]
    } else if v.is_kw(&kw.db_unique_identity) {
        &["db/unique", "db.unique/identity", "db/index"]
    } else if v.is_kw(&kw.db_unique_value) {
        &["db/unique", "db.unique/value", "db/index"]
    } else if v.is_kw(&kw.db_cardinality_many) {
        &["db.cardinality/many"]
    } else if v.is_kw(&kw.db_type_ref) {
        &["db.type/ref", "db/index"]
    } else if k.is_kw(&kw.db_is_component) && matches!(v, Value::Bool(true)) {
        &["db/isComponent"]
    } else if k.is_kw(&kw.db_index) && matches!(v, Value::Bool(true)) {
        &["db/index"]
    } else {
        &[]
    }
}

/// `rschema` of `(merge implicit-schema schema)`: each property's set of attributes, and each tuple source's tuples.
fn build_rschema(schema: &Value) -> (Value, HashMap<Attr, AttrProps>, Vec<Attr>) {
    let kw = kw();
    // {:db/ident {:db/unique :db.unique/identity}}, then the schema's own entries
    let mut merged = CljMap::new();
    merged.assoc(
        Value::Keyword(kw.db_ident.clone()),
        Value::kw_map(&[("db/unique", Value::Keyword(kw.db_unique_identity.clone()))]),
    );
    if let Value::Map(m) = schema {
        for (a, s) in m.iter() {
            merged.assoc(a.clone(), s.clone());
        }
    }

    let mut rs = CljMap::new();
    let mut props: HashMap<Attr, AttrProps> = HashMap::new();
    for (attr, attr_schema) in merged.iter() {
        let Value::Map(attr_schema) = attr_schema else { continue };
        for (k, v) in attr_schema.iter() {
            for prop in attr_properties(k, v) {
                let prop_kw = Value::kw(prop);
                let mut set = match rs.get(&prop_kw) {
                    Some(Value::Set(s)) => (**s).clone(),
                    _ => CljSet::new(),
                };
                set.insert(attr.clone());
                rs.assoc(prop_kw, Value::set(set));
                if let Some(a) = value_attr(attr) {
                    let p = props.entry(a).or_default();
                    match *prop {
                        "db.cardinality/many" => p.many = true,
                        "db.type/ref" => p.is_ref = true,
                        "db/unique" => p.unique = true,
                        "db.unique/identity" => p.unique_identity = true,
                        "db.unique/value" => p.unique_value = true,
                        "db/index" => p.index = true,
                        "db/isComponent" => p.component = true,
                        "db.type/tuple" => p.tuple = true,
                        "db/tupleAttrs" => p.composite = true,
                        _ => {}
                    }
                }
            }
        }
    }

    // each attribute's own tuple keys, from the schema as given
    if let Value::Map(m) = schema {
        for (attr, attr_schema) in m.iter() {
            let (Some(a), Value::Map(s)) = (value_attr(attr), attr_schema) else { continue };
            let get = |k: &Keyword| s.get(&Value::Keyword(k.clone())).cloned();
            let (ta, tt, t) = (get(&kw.db_tuple_attrs), get(&kw.db_tuple_types), get(&kw.db_tuple_type));
            if ta.is_some() || tt.is_some() || t.is_some() {
                let p = props.entry(a).or_default();
                p.tuple_attrs = ta;
                p.tuple_types = tt;
                p.tuple_type = t;
            }
        }
    }

    // :db/attrTuples: {source-attr {tuple-attr index}}
    let mut attr_tuples = CljMap::new();
    if let Some(Value::Set(tuples)) = rs.get(&Value::Keyword(kw.db_tuple_attrs.clone())) {
        for tuple_attr in tuples.iter() {
            let sources = schema.get(tuple_attr).and_then(|s| s.get_kw(&kw.db_tuple_attrs)).and_then(Value::seq_items);
            for (idx, src) in sources.unwrap_or_default().into_iter().enumerate() {
                let mut inner = match attr_tuples.get(&src) {
                    Some(Value::Map(m)) => (**m).clone(),
                    _ => CljMap::new(),
                };
                inner.assoc(tuple_attr.clone(), Value::from(idx));
                attr_tuples.assoc(src, Value::map(inner));
            }
        }
    }
    for (src, inner) in attr_tuples.iter() {
        let (Some(src), Value::Map(inner)) = (value_attr(src), inner) else { continue };
        let list = inner.iter().filter_map(|(t, idx)| Some((value_attr(t)?, idx.as_num()? as usize))).collect();
        props.entry(src).or_default().attr_tuples = list;
    }

    let ref_attrs = match rs.get(&Value::Keyword(kw.db_type_ref.clone())) {
        Some(Value::Set(s)) => s.iter().filter_map(value_attr).collect(),
        _ => Vec::new(),
    };
    rs.assoc(Value::Keyword(kw.db_attr_tuples.clone()), Value::map(attr_tuples));
    (Value::map(rs), props, ref_attrs)
}

fn validate_schema_key(a: &Value, k: &Keyword, v: Option<&Value>, expected: &[Value]) -> Result<()> {
    let Some(v) = v else { return Ok(()) };
    if v.is_nil() || expected.contains(v) {
        return Ok(());
    }
    let spec = Value::map(
        [(a.clone(), Value::map([(Value::Keyword(k.clone()), v.clone())].into_iter().collect()))].into_iter().collect(),
    );
    let expected_set: CljSet = expected.iter().cloned().collect();
    Err(Error::new(
        format!(
            "Bad attribute specification for {}, expected one of {}",
            pr_str(&spec),
            pr_str(&Value::set(expected_set))
        ),
        Value::kw_map(&[
            ("error", Value::kw("schema/validation")),
            ("attribute", a.clone()),
            ("key", Value::Keyword(k.clone())),
            ("value", v.clone()),
        ]),
    ))
}

/// `validate-schema`
fn validate_schema(schema: &Value) -> Result<()> {
    let kw = kw();
    let Value::Map(schema_map) = schema else { return Ok(()) };
    let key = |k: &Keyword| Value::Keyword(k.clone());
    for (a, kv) in schema_map.iter() {
        let get = |k: &Keyword| kv.get_kw(k);
        let has = |k: &Keyword| get(k).is_some();

        // isComponent
        let is_comp = get(&kw.db_is_component).is_some_and(Value::truthy);
        validate_schema_key(
            a,
            &kw.db_is_component,
            get(&kw.db_is_component),
            &[Value::Bool(true), Value::Bool(false)],
        )?;
        if is_comp && !get(&kw.db_value_type).is_some_and(|t| t.is_kw(&kw.db_type_ref)) {
            raise!("Bad attribute specification for ", a, ": {:db/isComponent true} should also have {:db/valueType :db.type/ref}";
                {"error" => Value::kw("schema/validation"), "attribute" => a.clone(), "key" => key(&kw.db_is_component)});
        }

        // The sets of what is allowed are literals in DataScript's source, which the ClojureScript compiler writes
        // out in the order Clojure's reader holds them: the order they print in here.
        validate_schema_key(
            a,
            &kw.db_unique,
            get(&kw.db_unique),
            &[key(&kw.db_unique_identity), key(&kw.db_unique_value)],
        )?;
        validate_schema_key(
            a,
            &kw.db_value_type,
            get(&kw.db_value_type),
            &[key(&kw.db_type_tuple), key(&kw.db_type_ref)],
        )?;
        validate_schema_key(
            a,
            &kw.db_cardinality,
            get(&kw.db_cardinality),
            &[key(&kw.db_cardinality_many), key(&kw.db_cardinality_one)],
        )?;

        // a tuple has exactly one of tupleAttrs, tupleTypes, tupleType
        let tuple_props: Vec<Value> = [&kw.db_tuple_attrs, &kw.db_tuple_types, &kw.db_tuple_type]
            .into_iter()
            .filter(|k| has(k))
            .map(key)
            .collect();
        if get(&kw.db_value_type).is_some_and(|t| t.is_kw(&kw.db_type_tuple)) && tuple_props.is_empty() {
            raise!("Bad attribute specification for ", a, ": {:db/valueType :db.type/tuple} should also have :db/tupleAttrs, :db/tupleTypes or :db/tupleType";
                {"error" => Value::kw("schema/validation"), "attribute" => a.clone(), "key" => key(&kw.db_value_type)});
        }
        if tuple_props.len() > 1 {
            raise!("Bad attribute specification for ", a, ": only one of :db/tupleAttrs, :db/tupleTypes, :db/tupleType is allowed, got ", Value::vector(tuple_props.clone());
                {"error" => Value::kw("schema/validation"), "attribute" => a.clone(), "key" => tuple_props[1].clone()});
        }

        // :db/tupleAttrs is a non-empty sequential collection
        if let Some(attrs) = get(&kw.db_tuple_attrs) {
            let ex_data = |value: Option<&Value>| {
                let mut pairs = vec![
                    ("error", Value::kw("schema/validation")),
                    ("attribute", a.clone()),
                    ("key", key(&kw.db_tuple_attrs)),
                ];
                if let Some(v) = value {
                    pairs.push(("value", v.clone()));
                }
                Value::kw_map(&pairs)
            };
            if get(&kw.db_cardinality).is_some_and(|c| c.is_kw(&kw.db_cardinality_many)) {
                return Err(Error::new(message!(a, " has :db/tupleAttrs, must be :db.cardinality/one"), ex_data(None)));
            }
            let Some(items) = attrs.as_seq() else {
                return Err(Error::new(
                    message!(a, " :db/tupleAttrs must be a sequential collection, got: ", attrs),
                    ex_data(None),
                ));
            };
            if items.is_empty() {
                return Err(Error::new(message!(a, " :db/tupleAttrs can’t be empty"), ex_data(None)));
            }
            for attr in items {
                let other = schema_map.get(attr);
                if other.is_some_and(|s| s.get_kw(&kw.db_tuple_attrs).is_some()) {
                    return Err(Error::new(
                        message!(a, " :db/tupleAttrs can’t depend on another tuple attribute: ", attr),
                        ex_data(Some(attr)),
                    ));
                }
                if other.and_then(|s| s.get_kw(&kw.db_cardinality)).is_some_and(|c| c.is_kw(&kw.db_cardinality_many)) {
                    return Err(Error::new(
                        message!(a, " :db/tupleAttrs can’t depend on :db.cardinality/many attribute: ", attr),
                        ex_data(Some(attr)),
                    ));
                }
            }
        }

        // :db/tupleTypes is a sequential collection of at least 2 keywords
        if let Some(types) = get(&kw.db_tuple_types) {
            let ok = types.as_seq().is_some_and(|ts| ts.len() >= 2 && ts.iter().all(Value::is_keyword));
            if !ok {
                raise!(a, " :db/tupleTypes must be a sequential collection of at least 2 keywords, got: ", types;
                    {"error" => Value::kw("schema/validation"), "attribute" => a.clone(), "key" => key(&kw.db_tuple_types)});
            }
        }

        // :db/tupleType is a keyword
        if let Some(t) = get(&kw.db_tuple_type) {
            if !t.is_keyword() {
                raise!(a, " :db/tupleType must be a keyword, got: ", t;
                    {"error" => Value::kw("schema/validation"), "attribute" => a.clone(), "key" => key(&kw.db_tuple_type)});
            }
        }
    }
    Ok(())
}
