//! `datascript.parser`: a query, its rules and its bindings, parsed into records and validated.
//!
//! The records are ClojureScript's, by name and field (`record`), and each function here is the original's: what it
//! accepts, what it answers, and the errors it raises.

use crate::clj;
use crate::coll::{CljMap, CljSet};
use crate::error::Result;
use crate::named::{Keyword, Symbol};
use crate::raise;
use crate::record::{as_record, is_record, record, record_with_source, Record};
use crate::value::Value;

const NS: &str = "datascript.parser";

fn rec(name: &'static str, fields: Vec<(&'static str, Value)>) -> Value {
    record(NS, name, fields)
}

fn rec_src(name: &'static str, fields: Vec<(&'static str, Value)>, source: &Value) -> Value {
    record_with_source(NS, name, fields, source)
}

fn vec_or_nil(v: Option<Vec<Value>>) -> Value {
    v.map_or(Value::Nil, Value::vector)
}

/// `(first (name sym))` of a symbol
fn sym_starts(form: &Value, c: char) -> bool {
    matches!(form, Value::Symbol(s) if s.name().starts_with(c))
}

fn is_sym(form: &Value, name: &str) -> bool {
    matches!(form, Value::Symbol(s) if s.full() == name)
}

/// `(of-size? form size)`
fn of_size(form: &Value, size: usize) -> bool {
    form.as_seq().is_some_and(|s| s.len() == size)
}

/// `parse-seq`: every element parsed, or `None` when the form is not sequential or an element does not parse.
fn parse_seq(parse_el: impl Fn(&Value) -> Result<Option<Value>>, form: &Value) -> Result<Option<Vec<Value>>> {
    let Some(items) = form.as_seq() else { return Ok(None) };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match parse_el(item)? {
            Some(parsed) => out.push(parsed),
            None => return Ok(None),
        }
    }
    Ok(Some(out))
}

/// `(next form)` of a sequential form: the rest as a list, or `nil` when there is none.
fn next_of(items: &[Value]) -> Value {
    if items.len() > 1 {
        Value::list(items[1..].to_vec())
    } else {
        Value::Nil
    }
}

/// `collect`: every part of a parsed form for which `pred` holds, records and collections walked field by field.
pub fn collect(pred: &dyn Fn(&Value) -> bool, form: &Value, acc: &mut Vec<Value>) {
    if pred(form) {
        acc.push(form.clone());
    } else if let Some(r) = as_record(form) {
        for (_, v) in &r.fields {
            collect(pred, v, acc);
        }
    } else if !form.is_nil() && form.is_seqable() {
        for x in form.seq_items().unwrap_or_default() {
            collect(pred, &x, acc);
        }
    }
}

/// `collect-vars-acc`: the variables of a parsed form. A `not` gives its own, an `or` its rule's.
fn collect_vars_acc(acc: &mut Vec<Value>, form: &Value) {
    if let Some(r) = as_record(form) {
        match r.name {
            "Variable" => acc.push(form.clone()),
            "Not" => acc.extend(r.get("vars").as_seq().unwrap_or(&[]).iter().cloned()),
            "Or" => collect_vars_acc(acc, r.get("rule-vars")),
            _ => {
                for (_, v) in &r.fields {
                    collect_vars_acc(acc, v);
                }
            }
        }
    } else if let Some(items) = form.as_seq() {
        for x in items {
            collect_vars_acc(acc, x);
        }
    }
}

fn collect_vars(form: &Value) -> Vec<Value> {
    let mut acc = Vec::new();
    collect_vars_acc(&mut acc, form);
    acc
}

/// `collect-vars-distinct`
pub fn collect_vars_distinct(form: &Value) -> Vec<Value> {
    clj::distinct(&collect_vars(form))
}

/// `distinct?` of a collection: no two equal.
fn all_distinct(items: &[Value]) -> bool {
    let set: CljSet = items.iter().cloned().collect();
    set.len() == items.len()
}

/// `(source obj)`: the form a record was parsed from, or the record.
pub fn source(obj: &Value) -> Value {
    as_record(obj).and_then(|r| r.source.clone()).unwrap_or_else(|| obj.clone())
}

fn symbol_of(r: &Record) -> Value {
    r.get("symbol").clone()
}

// ---------------------------------------------------------------- terms
// placeholder    = the symbol '_'
// variable       = symbol starting with "?"
// src-var        = symbol starting with "$"
// rules-var      = the symbol "%"
// constant       = any non-variable data literal
// plain-symbol   = symbol that does not begin with "$" or "?"

pub fn parse_placeholder(form: &Value) -> Option<Value> {
    is_sym(form, "_").then(|| rec("Placeholder", vec![]))
}

pub fn parse_variable(form: &Value) -> Option<Value> {
    sym_starts(form, '?').then(|| rec("Variable", vec![("symbol", form.clone())]))
}

fn parse_var_required(form: &Value) -> Result<Option<Value>> {
    match parse_variable(form) {
        Some(v) => Ok(Some(v)),
        None => raise!("Cannot parse var, expected symbol starting with ?, got: ", form;
            {"error" => Value::kw("parser/rule-var"), "form" => form.clone()}),
    }
}

pub fn parse_src_var(form: &Value) -> Option<Value> {
    sym_starts(form, '$').then(|| rec("SrcVar", vec![("symbol", form.clone())]))
}

pub fn parse_rules_var(form: &Value) -> Option<Value> {
    is_sym(form, "%").then(|| rec("RulesVar", vec![]))
}

pub fn parse_constant(form: &Value) -> Option<Value> {
    (!sym_starts(form, '?')).then(|| rec("Constant", vec![("value", form.clone())]))
}

pub fn parse_plain_symbol(form: &Value) -> Option<Value> {
    (form.is_symbol()
        && parse_variable(form).is_none()
        && parse_src_var(form).is_none()
        && parse_rules_var(form).is_none()
        && parse_placeholder(form).is_none())
    .then(|| rec("PlainSymbol", vec![("symbol", form.clone())]))
}

fn parse_plain_variable(form: &Value) -> Option<Value> {
    parse_plain_symbol(form).map(|_| rec("Variable", vec![("symbol", form.clone())]))
}

/// fn-arg = (variable | constant | src-var)
fn parse_fn_arg(form: &Value) -> Option<Value> {
    parse_variable(form).or_else(|| parse_src_var(form)).or_else(|| parse_constant(form))
}

// ---------------------------------------------------------------- rule vars
// rule-vars = [ variable+ | ([ variable+ ] variable*) ]

pub fn parse_rule_vars(form: &Value) -> Result<Value> {
    let Some(items) = form.as_seq() else {
        raise!("Cannot parse rule-vars, expected [ variable+ | ([ variable+ ] variable*) ]";
            {"error" => Value::kw("parser/rule-vars"), "form" => form.clone()})
    };
    let (required, rest) = match items.first() {
        Some(first) if first.is_sequential() => (first.clone(), next_of(items)),
        _ => (Value::Nil, form.clone()),
    };
    let required = parse_seq(parse_var_required, &required)?;
    let free = parse_seq(parse_var_required, &rest)?;
    let empty = |v: &Option<Vec<Value>>| v.as_ref().is_none_or(Vec::is_empty);
    if empty(&required) && empty(&free) {
        raise!("Cannot parse rule-vars, expected [ variable+ | ([ variable+ ] variable*) ]";
            {"error" => Value::kw("parser/rule-vars"), "form" => form.clone()})
    }
    let all: Vec<Value> = required.iter().flatten().chain(free.iter().flatten()).cloned().collect();
    if !all_distinct(&all) {
        raise!("Rule variables should be distinct";
            {"error" => Value::kw("parser/rule-vars"), "form" => form.clone()})
    }
    Ok(rec("RuleVars", vec![("required", vec_or_nil(required)), ("free", vec_or_nil(free))]))
}

/// `flatten-rule-vars`: `([required ...] free ...)`, as symbols
fn flatten_rule_vars(rule_vars: &Value) -> Value {
    let Some(r) = as_record(rule_vars) else { return Value::list(vec![]) };
    let symbols = |v: &Value| -> Vec<Value> {
        v.as_seq().unwrap_or(&[]).iter().filter_map(|x| as_record(x).map(symbol_of)).collect()
    };
    let mut out = Vec::new();
    if r.get("required").truthy() {
        out.push(Value::vector(symbols(r.get("required"))));
    }
    out.extend(symbols(r.get("free")));
    Value::list(out)
}

/// `rule-vars-arity`: how many required, how many free
fn rule_vars_arity(rule_vars: &Value) -> (usize, usize) {
    match as_record(rule_vars) {
        Some(r) => (r.get("required").count().unwrap_or(0), r.get("free").count().unwrap_or(0)),
        None => (0, 0),
    }
}

// ---------------------------------------------------------------- bindings
// binding        = (bind-scalar | bind-tuple | bind-coll | bind-rel)
// bind-scalar    = variable
// bind-tuple     = [ (binding | '_')+ ]
// bind-coll      = [ binding '...' ]
// bind-rel       = [ [ (binding | '_')+ ] ]

fn parse_bind_ignore(form: &Value) -> Option<Value> {
    is_sym(form, "_").then(|| rec_src("BindIgnore", vec![], form))
}

fn parse_bind_scalar(form: &Value) -> Option<Value> {
    parse_variable(form).map(|var| rec_src("BindScalar", vec![("variable", var)], form))
}

fn parse_bind_coll(form: &Value) -> Result<Option<Value>> {
    let Some(items) = form.as_seq() else { return Ok(None) };
    if items.len() == 2 && is_sym(&items[1], "...") {
        // a binding parses, or raises
        let sub = parse_binding(&items[0])?;
        return Ok(Some(rec_src("BindColl", vec![("binding", sub)], form)));
    }
    Ok(None)
}

fn parse_tuple_el(form: &Value) -> Result<Option<Value>> {
    match parse_bind_ignore(form) {
        Some(b) => Ok(Some(b)),
        None => parse_binding(form).map(Some),
    }
}

fn parse_bind_tuple(form: &Value) -> Result<Option<Value>> {
    match parse_seq(parse_tuple_el, form)? {
        None => Ok(None),
        Some(subs) if subs.is_empty() => raise!("Tuple binding cannot be empty";
            {"error" => Value::kw("parser/binding"), "form" => form.clone()}),
        Some(subs) => Ok(Some(rec_src("BindTuple", vec![("bindings", Value::vector(subs))], form))),
    }
}

fn parse_bind_rel(form: &Value) -> Result<Option<Value>> {
    let Some(items) = form.as_seq() else { return Ok(None) };
    if items.len() == 1 && items[0].is_sequential() {
        // a relation is a collection of tuples
        let tuple = parse_bind_tuple(&items[0])?.unwrap_or(Value::Nil);
        return Ok(Some(rec_src("BindColl", vec![("binding", tuple)], form)));
    }
    Ok(None)
}

pub fn parse_binding(form: &Value) -> Result<Value> {
    if let Some(b) = parse_bind_coll(form)? {
        return Ok(b);
    }
    if let Some(b) = parse_bind_rel(form)? {
        return Ok(b);
    }
    if let Some(b) = parse_bind_tuple(form)? {
        return Ok(b);
    }
    if let Some(b) = parse_bind_ignore(form).or_else(|| parse_bind_scalar(form)) {
        return Ok(b);
    }
    raise!("Cannot parse binding, expected (bind-scalar | bind-tuple | bind-coll | bind-rel)";
        {"error" => Value::kw("parser/binding"), "form" => form.clone()})
}

// ---------------------------------------------------------------- find
// find-spec        = ':find' (find-rel | find-coll | find-tuple | find-scalar)
// find-rel         = find-elem+
// find-coll        = [ find-elem '...' ]
// find-scalar      = find-elem '.'
// find-tuple       = [ find-elem+ ]
// find-elem        = (variable | pull-expr | aggregate | custom-aggregate)
// pull-expr        = [ 'pull' src-var? variable pull-pattern ]
// pull-pattern     = (constant | variable | plain-symbol)
// aggregate        = [ aggregate-fn fn-arg+ ]
// aggregate-fn     = plain-symbol
// custom-aggregate = [ 'aggregate' variable fn-arg+ ]

/// `find-elements`
pub fn find_elements(find: &Value) -> Vec<Value> {
    let Some(r) = as_record(find) else { return Vec::new() };
    match r.name {
        "FindRel" | "FindTuple" => r.get("elements").as_seq().unwrap_or(&[]).to_vec(),
        _ => vec![r.get("element").clone()],
    }
}

/// `-find-vars` of an element: the variable's symbol, a pull's variable's, an aggregate's last argument's.
fn element_find_vars(element: &Value) -> Vec<Value> {
    let Some(r) = as_record(element) else { return Vec::new() };
    match r.name {
        "Variable" => vec![symbol_of(r)],
        "Aggregate" => r.get("args").as_seq().and_then(<[Value]>::last).map(element_find_vars).unwrap_or_default(),
        "Pull" => element_find_vars(r.get("variable")),
        _ => Vec::new(),
    }
}

/// `find-vars`
pub fn find_vars(find: &Value) -> Vec<Value> {
    find_elements(find).iter().flat_map(element_find_vars).collect()
}

pub fn is_aggregate(element: &Value) -> bool {
    is_record(element, "Aggregate")
}

pub fn is_pull(element: &Value) -> bool {
    is_record(element, "Pull")
}

fn parse_aggregate(form: &Value) -> Result<Option<Value>> {
    let Some(items) = form.as_seq() else { return Ok(None) };
    if items.len() < 2 {
        return Ok(None);
    }
    let f = parse_plain_symbol(&items[0]);
    let args = parse_seq(|x| Ok(parse_fn_arg(x)), &next_of(items))?;
    Ok(match (f, args) {
        (Some(f), Some(args)) => Some(rec("Aggregate", vec![("fn", f), ("args", Value::vector(args))])),
        _ => None,
    })
}

fn parse_aggregate_custom(form: &Value) -> Result<Option<Value>> {
    let Some(items) = form.as_seq() else { return Ok(None) };
    if !items.first().is_some_and(|f| is_sym(f, "aggregate")) {
        return Ok(None);
    }
    if items.len() >= 3 {
        let f = parse_variable(&items[1]);
        let args = parse_seq(|x| Ok(parse_fn_arg(x)), &Value::list(items[2..].to_vec()))?;
        if let (Some(f), Some(args)) = (f, args) {
            return Ok(Some(rec("Aggregate", vec![("fn", f), ("args", Value::vector(args))])));
        }
    }
    raise!("Cannot parse custom aggregate call, expect ['aggregate' variable fn-arg+]";
        {"error" => Value::kw("parser/find"), "fragment" => form.clone()})
}

fn parse_pull_expr(form: &Value) -> Result<Option<Value>> {
    let Some(items) = form.as_seq() else { return Ok(None) };
    if !items.first().is_some_and(|f| is_sym(f, "pull")) {
        return Ok(None);
    }
    if (3..=4).contains(&items.len()) {
        let long = items.len() == 4;
        let src = if long { items[1].clone() } else { Value::sym("$") };
        let (var, pattern) = if long { (&items[2], &items[3]) } else { (&items[1], &items[2]) };
        let src = parse_src_var(&src);
        let var = parse_variable(var);
        let pattern =
            parse_variable(pattern).or_else(|| parse_plain_variable(pattern)).or_else(|| parse_constant(pattern));
        if let (Some(src), Some(var), Some(pattern)) = (src, var, pattern) {
            return Ok(Some(rec("Pull", vec![("source", src), ("variable", var), ("pattern", pattern)])));
        }
    }
    raise!("Cannot parse pull expression, expect ['pull' src-var? variable (constant | variable | plain-symbol)]";
        {"error" => Value::kw("parser/find"), "fragment" => form.clone()})
}

fn parse_find_elem(form: &Value) -> Result<Option<Value>> {
    if let Some(v) = parse_variable(form) {
        return Ok(Some(v));
    }
    if let Some(p) = parse_pull_expr(form)? {
        return Ok(Some(p));
    }
    if let Some(a) = parse_aggregate_custom(form)? {
        return Ok(Some(a));
    }
    parse_aggregate(form)
}

fn parse_find_rel(form: &Value) -> Result<Option<Value>> {
    Ok(parse_seq(parse_find_elem, form)?.map(|els| rec("FindRel", vec![("elements", Value::vector(els))])))
}

fn parse_find_coll(form: &Value) -> Result<Option<Value>> {
    let Some(items) = form.as_seq() else { return Ok(None) };
    if items.len() != 1 {
        return Ok(None);
    }
    match items[0].as_seq() {
        Some(inner) if inner.len() == 2 && is_sym(&inner[1], "...") => {
            Ok(parse_find_elem(&inner[0])?.map(|el| rec("FindColl", vec![("element", el)])))
        }
        _ => Ok(None),
    }
}

fn parse_find_scalar(form: &Value) -> Result<Option<Value>> {
    match form.as_seq() {
        Some(items) if items.len() == 2 && is_sym(&items[1], ".") => {
            Ok(parse_find_elem(&items[0])?.map(|el| rec("FindScalar", vec![("element", el)])))
        }
        _ => Ok(None),
    }
}

fn parse_find_tuple(form: &Value) -> Result<Option<Value>> {
    match form.as_seq() {
        Some(items) if items.len() == 1 => {
            Ok(parse_seq(parse_find_elem, &items[0])?
                .map(|els| rec("FindTuple", vec![("elements", Value::vector(els))])))
        }
        _ => Ok(None),
    }
}

pub fn parse_find(form: &Value) -> Result<Value> {
    if let Some(f) = parse_find_rel(form)? {
        return Ok(f);
    }
    if let Some(f) = parse_find_coll(form)? {
        return Ok(f);
    }
    if let Some(f) = parse_find_scalar(form)? {
        return Ok(f);
    }
    if let Some(f) = parse_find_tuple(form)? {
        return Ok(f);
    }
    raise!("Cannot parse :find, expected: (find-rel | find-coll | find-tuple | find-scalar)";
        {"error" => Value::kw("parser/find"), "fragment" => form.clone()})
}

// ---------------------------------------------------------------- return maps, with, in
// return-map  = (return-keys | return-syms | return-strs)
// return-keys = ':keys' symbol+
// return-syms = ':syms' symbol+
// return-strs = ':strs' symbol+

pub fn parse_return_map(ty: &str, form: Option<&Value>) -> Option<Value> {
    let items = form?.seq_items()?;
    if items.is_empty() || !items.iter().all(Value::is_symbol) {
        return None;
    }
    let symbols: Vec<Value> = match ty {
        "keys" => {
            items.iter().filter_map(|s| s.as_symbol().map(|s| Value::Keyword(Keyword::new(s.ns(), s.name())))).collect()
        }
        "syms" => items,
        "strs" => items.iter().map(|s| Value::from(crate::print::str_of(s))).collect(),
        _ => return None,
    };
    Some(rec("ReturnMap", vec![("type", Value::kw(ty)), ("symbols", Value::vector(symbols))]))
}

/// with = [ variable+ ]
pub fn parse_with(form: &Value) -> Result<Vec<Value>> {
    match parse_seq(|x| Ok(parse_variable(x)), form)? {
        Some(vars) => Ok(vars),
        None => raise!("Cannot parse :with clause, expected [ variable+ ]";
            {"error" => Value::kw("parser/with"), "form" => form.clone()}),
    }
}

/// in = [ (src-var | rules-var | plain-symbol | binding)+ ]
fn parse_in_binding(form: &Value) -> Result<Option<Value>> {
    match parse_src_var(form).or_else(|| parse_rules_var(form)).or_else(|| parse_plain_variable(form)) {
        Some(var) => Ok(Some(rec_src("BindScalar", vec![("variable", var)], form))),
        None => parse_binding(form).map(Some),
    }
}

pub fn parse_in(form: &Value) -> Result<Vec<Value>> {
    match parse_seq(parse_in_binding, form)? {
        Some(bindings) => Ok(bindings),
        None => {
            raise!("Cannot parse :in clause, expected (src-var | % | plain-symbol | bind-scalar | bind-tuple | bind-coll | bind-rel)";
            {"error" => Value::kw("parser/in"), "form" => form.clone()})
        }
    }
}

// ---------------------------------------------------------------- clauses
// clause          = (data-pattern | pred-expr | fn-expr | rule-expr | not-clause | not-join-clause | or-clause | or-join-clause)
// data-pattern    = [ src-var? (variable | constant | '_')+ ]
// pred-expr       = [ [ pred fn-arg+ ] ]
// pred            = (plain-symbol | variable)
// fn-expr         = [ [ fn fn-arg+ ] binding ]
// fn              = (plain-symbol | variable)
// rule-expr       = [ src-var? rule-name (variable | constant | '_')+ ]
// not-clause      = [ src-var? 'not' clause+ ]
// not-join-clause = [ src-var? 'not-join' [ variable+ ] clause+ ]
// or-clause       = [ src-var? 'or' (clause | and-clause)+ ]
// or-join-clause  = [ src-var? 'or-join' rule-vars (clause | and-clause)+ ]
// and-clause      = [ 'and' clause+ ]

fn parse_pattern_el(form: &Value) -> Option<Value> {
    parse_placeholder(form).or_else(|| parse_variable(form)).or_else(|| parse_constant(form))
}

/// `take-source`: a form's source, its own or the default, and the rest of it.
fn take_source(form: &Value) -> Option<(Value, Value)> {
    let items = form.as_seq()?;
    match items.first().and_then(parse_src_var) {
        Some(source) => Some((source, next_of(items))),
        None => Some((rec("DefaultSrc", vec![]), form.clone())),
    }
}

fn parse_pattern(form: &Value) -> Result<Option<Value>> {
    let Some((source, next_form)) = take_source(form) else { return Ok(None) };
    match parse_seq(|x| Ok(parse_pattern_el(x)), &next_form)? {
        None => Ok(None),
        Some(pattern) if pattern.is_empty() => raise!("Pattern could not be empty";
            {"error" => Value::kw("parser/where"), "form" => form.clone()}),
        Some(pattern) => {
            Ok(Some(rec_src("Pattern", vec![("source", source), ("pattern", Value::vector(pattern))], form)))
        }
    }
}

/// `parse-call`: a function, plain or bound to a variable, and its arguments.
fn parse_call(form: &Value) -> Result<Option<(Value, Vec<Value>)>> {
    let Some(items) = form.as_seq() else { return Ok(None) };
    let f = items.first().and_then(|f| parse_plain_symbol(f).or_else(|| parse_variable(f)));
    let args = if items.len() > 1 { Value::list(items[1..].to_vec()) } else { Value::vector(vec![]) };
    let args = parse_seq(|x| Ok(parse_fn_arg(x)), &args)?;
    Ok(match (f, args) {
        (Some(f), Some(args)) => Some((f, args)),
        _ => None,
    })
}

fn parse_pred(form: &Value) -> Result<Option<Value>> {
    if !of_size(form, 1) {
        return Ok(None);
    }
    let call = &form.as_seq().unwrap()[0];
    Ok(parse_call(call)?.map(|(f, args)| rec_src("Predicate", vec![("fn", f), ("args", Value::vector(args))], form)))
}

fn parse_fn(form: &Value) -> Result<Option<Value>> {
    if !of_size(form, 2) {
        return Ok(None);
    }
    let items = form.as_seq().unwrap();
    let Some((f, args)) = parse_call(&items[0])? else { return Ok(None) };
    let binding = parse_binding(&items[1])?;
    Ok(Some(rec_src("Function", vec![("fn", f), ("args", Value::vector(args)), ("binding", binding)], form)))
}

fn parse_rule_expr(form: &Value) -> Result<Option<Value>> {
    let Some((source, next_form)) = take_source(form) else { return Ok(None) };
    let items = next_form.as_seq().unwrap_or(&[]);
    let Some(name) = items.first().and_then(parse_plain_symbol) else { return Ok(None) };
    let args = next_of(items);
    if args.is_nil() {
        raise!("rule-expr requires at least one argument";
            {"error" => Value::kw("parser/where"), "form" => form.clone()})
    }
    match parse_seq(|x| Ok(parse_pattern_el(x)), &args)? {
        None => raise!("Cannot parse rule-expr arguments, expected [ (variable | constant | '_')+ ]";
            {"error" => Value::kw("parser/where"), "form" => form.clone()}),
        Some(args) => {
            Ok(Some(rec("RuleExpr", vec![("source", source), ("name", name), ("args", Value::vector(args))])))
        }
    }
}

/// `validate-join-vars`
fn validate_join_vars(required: &Value, free: &Value, form: &Value) -> Result<()> {
    let empty = |v: &Value| v.count().unwrap_or(0) == 0;
    if empty(required) && empty(free) {
        raise!("Join variables should not be empty";
            {"error" => Value::kw("parser/where"), "form" => form.clone()})
    }
    Ok(())
}

fn parse_not(form: &Value) -> Result<Option<Value>> {
    let Some((source, next_form)) = take_source(form) else { return Ok(None) };
    let items = next_form.as_seq().unwrap_or(&[]);
    if !items.first().is_some_and(|s| is_sym(s, "not")) {
        return Ok(None);
    }
    match parse_clauses(&next_of(items))? {
        Some(clauses) => {
            let clauses = Value::vector(clauses);
            let vars = Value::vector(collect_vars_distinct(&clauses));
            validate_join_vars(&Value::Nil, &vars, form)?;
            Ok(Some(rec_src("Not", vec![("source", source), ("vars", vars), ("clauses", clauses)], form)))
        }
        None => raise!("Cannot parse 'not' clause, expected [ src-var? 'not' clause+ ]";
            {"error" => Value::kw("parser/where"), "form" => form.clone()}),
    }
}

fn parse_not_join(form: &Value) -> Result<Option<Value>> {
    let Some((source, next_form)) = take_source(form) else { return Ok(None) };
    let items = next_form.as_seq().unwrap_or(&[]);
    if !items.first().is_some_and(|s| is_sym(s, "not-join")) {
        return Ok(None);
    }
    let vars = parse_seq(|x| Ok(parse_variable(x)), items.get(1).unwrap_or(&Value::Nil))?;
    let clauses = parse_clauses(&if items.len() > 2 { Value::list(items[2..].to_vec()) } else { Value::Nil })?;
    match (vars, clauses) {
        (Some(vars), Some(clauses)) => {
            let vars = Value::vector(vars);
            validate_join_vars(&Value::Nil, &vars, form)?;
            Ok(Some(rec_src(
                "Not",
                vec![("source", source), ("vars", vars), ("clauses", Value::vector(clauses))],
                form,
            )))
        }
        _ => raise!("Cannot parse 'not-join' clause, expected [ src-var? 'not-join' [variable+] clause+ ]";
            {"error" => Value::kw("parser/where"), "form" => form.clone()}),
    }
}

fn validate_or(clause: Value, form: &Value) -> Result<Value> {
    let (required, free) = match as_record(&clause).and_then(|r| as_record(r.get("rule-vars"))) {
        Some(rv) => (rv.get("required").clone(), rv.get("free").clone()),
        None => (Value::Nil, Value::Nil),
    };
    validate_join_vars(&required, &free, form)?;
    Ok(clause)
}

fn parse_and(form: &Value) -> Result<Option<Value>> {
    let Some(items) = form.as_seq() else { return Ok(None) };
    if !items.first().is_some_and(|s| is_sym(s, "and")) {
        return Ok(None);
    }
    match parse_clauses(&next_of(items))? {
        Some(clauses) if !clauses.is_empty() => Ok(Some(rec("And", vec![("clauses", Value::vector(clauses))]))),
        _ => raise!("Cannot parse 'and' clause, expected [ 'and' clause+ ]";
            {"error" => Value::kw("parser/where"), "form" => form.clone()}),
    }
}

/// `(some-fn parse-and parse-clause)`
fn parse_and_or_clause(form: &Value) -> Result<Option<Value>> {
    match parse_and(form)? {
        Some(a) => Ok(Some(a)),
        None => parse_clause(form).map(Some),
    }
}

fn parse_or(form: &Value) -> Result<Option<Value>> {
    let Some((source, next_form)) = take_source(form) else { return Ok(None) };
    let items = next_form.as_seq().unwrap_or(&[]);
    if !items.first().is_some_and(|s| is_sym(s, "or")) {
        return Ok(None);
    }
    match parse_seq(parse_and_or_clause, &next_of(items))? {
        Some(clauses) => {
            let clauses = Value::vector(clauses);
            let rule_vars = rec(
                "RuleVars",
                vec![("required", Value::Nil), ("free", Value::vector(collect_vars_distinct(&clauses)))],
            );
            let clause = rec_src("Or", vec![("source", source), ("rule-vars", rule_vars), ("clauses", clauses)], form);
            validate_or(clause, form).map(Some)
        }
        None => raise!("Cannot parse 'or' clause, expected [ src-var? 'or' clause+ ]";
            {"error" => Value::kw("parser/where"), "form" => form.clone()}),
    }
}

fn parse_or_join(form: &Value) -> Result<Option<Value>> {
    let Some((source, next_form)) = take_source(form) else { return Ok(None) };
    let items = next_form.as_seq().unwrap_or(&[]);
    if !items.first().is_some_and(|s| is_sym(s, "or-join")) {
        return Ok(None);
    }
    let vars = parse_rule_vars(items.get(1).unwrap_or(&Value::Nil))?;
    let clauses =
        parse_seq(parse_and_or_clause, &if items.len() > 2 { Value::list(items[2..].to_vec()) } else { Value::Nil })?;
    match clauses {
        Some(clauses) => {
            let clause =
                rec_src("Or", vec![("source", source), ("rule-vars", vars), ("clauses", Value::vector(clauses))], form);
            validate_or(clause, form).map(Some)
        }
        None => raise!("Cannot parse 'or-join' clause, expected [ src-var? 'or-join' [variable+] clause+ ]";
            {"error" => Value::kw("parser/where"), "form" => form.clone()}),
    }
}

pub fn parse_clause(form: &Value) -> Result<Value> {
    if let Some(c) = parse_not(form)? {
        return Ok(c);
    }
    if let Some(c) = parse_not_join(form)? {
        return Ok(c);
    }
    if let Some(c) = parse_or(form)? {
        return Ok(c);
    }
    if let Some(c) = parse_or_join(form)? {
        return Ok(c);
    }
    if let Some(c) = parse_pred(form)? {
        return Ok(c);
    }
    if let Some(c) = parse_fn(form)? {
        return Ok(c);
    }
    if let Some(c) = parse_rule_expr(form)? {
        return Ok(c);
    }
    if let Some(c) = parse_pattern(form)? {
        return Ok(c);
    }
    raise!("Cannot parse clause, expected (data-pattern | pred-expr | fn-expr | rule-expr | not-clause | not-join-clause | or-clause | or-join-clause)";
        {"error" => Value::kw("parser/where"), "form" => form.clone()})
}

pub fn parse_clauses(clauses: &Value) -> Result<Option<Vec<Value>>> {
    parse_seq(|c| parse_clause(c).map(Some), clauses)
}

pub fn parse_where(form: &Value) -> Result<Vec<Value>> {
    match parse_clauses(form)? {
        Some(clauses) => Ok(clauses),
        None => raise!("Cannot parse :where clause, expected [clause+]";
            {"error" => Value::kw("parser/where"), "form" => form.clone()}),
    }
}

// ---------------------------------------------------------------- rules
// rule-branch = [rule-head clause+]
// rule-head   = [rule-name rule-vars]
// rule-name   = plain-symbol

/// `parse-rule`: a branch's name, vars and clauses.
fn parse_rule(form: &Value) -> Result<(Value, Value, Value)> {
    let Some(items) = form.as_seq() else {
        raise!("Cannot parse rule, expected [rule-head clause+]";
            {"error" => Value::kw("parser/rule"), "form" => form.clone()})
    };
    let head = items.first().cloned().unwrap_or(Value::Nil);
    let Some(head_items) = head.as_seq() else {
        raise!("Cannot parse rule head, expected [rule-name rule-vars], got: ", head;
            {"error" => Value::kw("parser/rule"), "form" => form.clone()})
    };
    let Some(name) = head_items.first().and_then(parse_plain_symbol) else {
        raise!("Cannot parse rule name, expected plain-symbol";
            {"error" => Value::kw("parser/rule"), "form" => form.clone()})
    };
    let vars = parse_rule_vars(&next_of(head_items))?;
    let clauses = match parse_clauses(&next_of(items))? {
        Some(clauses) if !clauses.is_empty() => Value::vector(clauses),
        _ => raise!("Rule branch should have clauses";
            {"error" => Value::kw("parser/rule"), "form" => form.clone()}),
    };
    Ok((name, vars, clauses))
}

/// `parse-rules`: the rules, each with its branches, which must agree on arity.
pub fn parse_rules(form: &Value) -> Result<Value> {
    let Some(items) = form.as_seq() else { return Ok(Value::vector(vec![])) };
    // (group-by :name ...): the names in the order of ClojureScript's map of them
    let mut by_name = CljMap::new();
    for item in items {
        let (name, vars, clauses) = parse_rule(item)?;
        let mut branches = match by_name.get(&name) {
            Some(Value::Vector(b)) => b.items().to_vec(),
            _ => Vec::new(),
        };
        branches.push(rec("RuleBranch", vec![("vars", vars), ("clauses", clauses)]));
        by_name.assoc(name, Value::vector(branches));
    }
    let mut rules = Vec::new();
    for (name, branches) in by_name.iter() {
        let branch_list = branches.as_seq().unwrap_or(&[]);
        // validate-arity
        if let Some(first) = branch_list.first() {
            let vars0 = as_record(first).map(|r| r.get("vars").clone()).unwrap_or(Value::Nil);
            let arity0 = rule_vars_arity(&vars0);
            for b in &branch_list[1..] {
                let vars = as_record(b).map(|r| r.get("vars").clone()).unwrap_or(Value::Nil);
                if rule_vars_arity(&vars) != arity0 {
                    let symbol = as_record(name).map(symbol_of).unwrap_or(Value::Nil);
                    raise!("Arity mismatch for rule '", symbol, "': ", flatten_rule_vars(&vars0), " vs. ", flatten_rule_vars(&vars);
                        {"error" => Value::kw("parser/rule"), "rule" => name.clone()})
                }
            }
        }
        rules.push(rec("Rule", vec![("name", name.clone()), ("branches", branches.clone())]));
    }
    Ok(Value::vector(rules))
}

// ---------------------------------------------------------------- query

/// `query->map`: `[:find ?e :where [...]]` as `{:find [?e], :where [[...]]}`. As in the original, it reads up to
/// the first `nil` or `false`.
pub fn query_to_map(query: &[Value]) -> CljMap {
    let mut parsed = CljMap::new();
    let mut key = Value::Nil;
    for q in query {
        if !q.truthy() {
            break;
        }
        if q.is_keyword() {
            key = q.clone();
        } else {
            let mut items = match parsed.get(&key) {
                Some(Value::Vector(v)) => v.items().to_vec(),
                _ => Vec::new(),
            };
            items.push(q.clone());
            parsed.assoc(key.clone(), Value::vector(items));
        }
    }
    parsed
}

/// `default-in`: a query whose `:where` reads a source takes `$` unless it says otherwise.
fn default_in(qwhere: &Value) -> Value {
    let explicit_input = |parsed: &Value| -> bool {
        let Some(r) = as_record(parsed) else { return false };
        let source = r.get("source");
        if r.is("Pattern") {
            source.truthy()
        } else {
            source.is_some() && !is_record(source, "DefaultSrc")
        }
    };
    let mut found = Vec::new();
    collect(&explicit_input, qwhere, &mut found);
    if found.is_empty() {
        Value::vector(vec![])
    } else {
        Value::vector(vec![Value::sym("$")])
    }
}

/// `clojure.set/union`, which adds the smaller set to the larger
fn set_union(a: &CljSet, b: &CljSet) -> CljSet {
    let (mut into, from) = if a.len() < b.len() { (b.clone(), a) } else { (a.clone(), b) };
    for x in from.iter() {
        into.insert(x.clone());
    }
    into
}

/// `clojure.set/difference`
fn set_difference(a: &CljSet, b: &CljSet) -> CljSet {
    let mut out = a.clone();
    if a.len() < b.len() {
        for x in a.iter() {
            if b.contains(x) {
                out.remove(x);
            }
        }
    } else {
        for x in b.iter() {
            out.remove(x);
        }
    }
    out
}

/// `clojure.set/intersection`
fn set_intersection(a: &CljSet, b: &CljSet) -> CljSet {
    let (a, b) = if b.len() < a.len() { (b, a) } else { (a, b) };
    let mut out = a.clone();
    for x in a.iter() {
        if !b.contains(x) {
            out.remove(x);
        }
    }
    out
}

fn symbols_of(set: &CljSet) -> Value {
    Value::vector(set.iter().filter_map(|v| as_record(v).map(symbol_of)).collect())
}

fn validate_query(q: &Record, form: &Value, form_map: &CljMap) -> Result<()> {
    let to_set = |items: Vec<Value>| -> CljSet { items.into_iter().collect() };
    let find_vars = to_set(collect_vars(q.get("qfind")));
    let with_vars = to_set(q.get("qwith").as_seq().unwrap_or(&[]).to_vec());
    let in_vars = to_set(collect_vars(q.get("qin")));
    let where_vars = to_set(collect_vars(q.get("qwhere")));
    let unknown = set_difference(&set_union(&find_vars, &with_vars), &set_union(&where_vars, &in_vars));
    let shared = set_intersection(&find_vars, &with_vars);
    if !unknown.is_empty() {
        raise!("Query for unknown vars: ", symbols_of(&unknown);
            {"error" => Value::kw("parser/query"), "vars" => Value::set(unknown.clone()), "form" => form.clone()})
    }
    if !shared.is_empty() {
        raise!(":find and :with should not use same variables: ", symbols_of(&shared);
            {"error" => Value::kw("parser/query"), "vars" => Value::set(shared.clone()), "form" => form.clone()})
    }

    let return_map = q.get("qreturn-map");
    if let Some(rm) = as_record(return_map) {
        if is_record(q.get("qfind"), "FindScalar") {
            raise!(rm.get("type"), " does not work with single-scalar :find";
                {"error" => Value::kw("parser/query"), "form" => form.clone()})
        }
        if is_record(q.get("qfind"), "FindColl") {
            raise!(rm.get("type"), " does not work with collection :find";
                {"error" => Value::kw("parser/query"), "form" => form.clone()})
        }
        if let Some(return_symbols) = rm.get("symbols").as_seq() {
            let elements = find_elements(q.get("qfind"));
            if return_symbols.len() != elements.len() {
                let mut rm_form = vec![rm.get("type").clone()];
                rm_form.extend(return_symbols.iter().cloned());
                raise!("Count of ", rm.get("type"), " must match count of :find";
                    {"error" => Value::kw("parser/query"), "return-map" => Value::list(rm_form),
                     "find" => Value::vector(elements), "form" => form.clone()})
            }
        }
    }

    let present =
        ["keys", "syms", "strs"].iter().filter(|k| form_map.get(&Value::kw(k)).is_some_and(Value::is_some)).count();
    if present > 1 {
        raise!("Only one of :keys/:syms/:strs must be present";
            {"error" => Value::kw("parser/query"), "form" => form.clone()})
    }

    let collect_all = |pred: &dyn Fn(&Value) -> bool, v: &Value| -> Vec<Value> {
        let mut acc = Vec::new();
        collect(pred, v, &mut acc);
        acc
    };
    let is_src = |v: &Value| is_record(v, "SrcVar");
    let is_rules = |v: &Value| is_record(v, "RulesVar");
    let in_sources = collect_all(&is_src, q.get("qin"));
    let in_rules = collect_all(&is_rules, q.get("qin"));
    if !(all_distinct(&collect_vars(q.get("qin"))) && all_distinct(&in_sources) && all_distinct(&in_rules)) {
        raise!("Vars used in :in should be distinct";
            {"error" => Value::kw("parser/query"), "form" => form.clone()})
    }
    if !all_distinct(&collect_vars(q.get("qwith"))) {
        raise!("Vars used in :with should be distinct";
            {"error" => Value::kw("parser/query"), "form" => form.clone()})
    }

    let where_sources = to_set(collect_all(&is_src, q.get("qwhere")));
    let unknown = set_difference(&where_sources, &to_set(in_sources));
    if !unknown.is_empty() {
        raise!("Where uses unknown source vars: ", symbols_of(&unknown);
            {"error" => Value::kw("parser/query"), "vars" => Value::set(unknown.clone()), "form" => form.clone()})
    }

    let rule_exprs = collect_all(&|v| is_record(v, "RuleExpr"), q.get("qwhere"));
    if !rule_exprs.is_empty() && in_rules.is_empty() {
        raise!("Missing rules var '%' in :in";
            {"error" => Value::kw("parser/query"), "form" => form.clone()})
    }
    Ok(())
}

/// `parse-query`: a `Query` record of a query given as a vector or a map.
pub fn parse_query(q: &Value) -> Result<Value> {
    let qm: CljMap = match q {
        Value::Map(m) => (**m).clone(),
        Value::Vector(items) | Value::List(items) => query_to_map(items),
        _ => raise!("Query should be a vector or a map";
            {"error" => Value::kw("parser/query"), "form" => q.clone()}),
    };
    let get = |k: &str| qm.get(&Value::kw(k));
    let qwhere = Value::vector(parse_where(get("where").unwrap_or(&Value::vector(vec![])))?);
    let qfind = parse_find(get("find").unwrap_or(&Value::Nil))?;
    let qwith = match get("with").filter(|w| w.truthy()) {
        Some(with) => Value::vector(parse_with(with)?),
        None => Value::Nil,
    };
    let qreturn_map = parse_return_map("keys", get("keys"))
        .or_else(|| parse_return_map("syms", get("syms")))
        .or_else(|| parse_return_map("strs", get("strs")))
        .unwrap_or(Value::Nil);
    let qin = match get("in").filter(|i| i.truthy()) {
        Some(qin) => parse_in(qin)?,
        None => parse_in(&default_in(&qwhere))?,
    };
    let res = rec(
        "Query",
        vec![
            ("qfind", qfind),
            ("qwith", qwith),
            ("qreturn-map", qreturn_map),
            ("qin", Value::vector(qin)),
            ("qwhere", qwhere),
        ],
    );
    validate_query(as_record(&res).expect("a record"), q, &qm)?;
    Ok(res)
}

/// The symbol of a `Variable`, `SrcVar` or `PlainSymbol` record.
pub fn record_symbol(v: &Value) -> Option<Symbol> {
    as_record(v).and_then(|r| r.get("symbol").as_symbol().cloned())
}
