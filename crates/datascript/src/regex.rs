//! Regular expressions for `re-find`, `re-matches`, `re-seq` and `re-pattern`: JavaScript's syntax and its
//! backtracking semantics, since those are what a ClojureScript query's patterns are written for.
//!
//! A host that has JavaScript's own `RegExp` can put it in place of this one (`set_engine`). This engine reads the
//! common syntax: alternation, groups (capturing, named, non-capturing), greedy and lazy quantifiers, classes and
//! their escapes, anchors, word boundaries, backreferences and lookahead; the flags `i`, `m` and `s`. It matches over
//! characters, where JavaScript matches over UTF-16 code units; the two differ only on characters beyond U+FFFF.
//! Lookbehind is not here.

use crate::error::{Error, Result};
use crate::value::Regex;
use std::sync::{Arc, OnceLock, RwLock};

/// What `RegExp.prototype.exec` answers: where the match starts, in UTF-16 code units, and the groups, the whole
/// match first.
pub struct Match {
    pub index: usize,
    pub groups: Vec<Option<String>>,
}

type Engine = dyn Fn(&str, &str, &str) -> Result<Option<Match>> + Send + Sync;

fn engine() -> &'static RwLock<Option<Arc<Engine>>> {
    static ENGINE: OnceLock<RwLock<Option<Arc<Engine>>>> = OnceLock::new();
    ENGINE.get_or_init(Default::default)
}

/// Puts the host's regular expressions in place of this module's: `f(source, flags, input)`.
pub fn set_engine(f: Option<Arc<Engine>>) {
    *engine().write().unwrap_or_else(|e| e.into_inner()) = f;
}

/// `re.exec(input)`
pub fn exec(re: &Regex, input: &str) -> Result<Option<Match>> {
    if let Some(host) = engine().read().unwrap_or_else(|e| e.into_inner()).clone() {
        return host(&re.source, &re.flags, input);
    }
    let program = re
        .program
        .get_or_init(|| compile(&re.source, &re.flags).map(Arc::new))
        .as_ref()
        .map_err(|e| Error::msg(format!("Invalid regular expression: /{}/: {e}", re.source)))?;
    program.exec(input)
}

/// Whether the source is a regular expression at all: what `new RegExp` checks when it makes one.
pub fn validate(re: &Regex) -> Result<()> {
    if engine().read().unwrap_or_else(|e| e.into_inner()).is_some() {
        return exec(re, "").map(|_| ());
    }
    re.program
        .get_or_init(|| compile(&re.source, &re.flags).map(Arc::new))
        .as_ref()
        .map(|_| ())
        .map_err(|e| Error::msg(format!("Invalid regular expression: /{}/: {e}", re.source)))
}

// ---------------------------------------------------------------- syntax

#[derive(Clone, Debug)]
enum Node {
    Empty,
    Char(char),
    Any,
    Class(Vec<ClassItem>, bool),
    Start,
    End,
    WordBoundary(bool),
    Group(Box<Node>, Option<usize>),
    Backref(usize),
    Look(Box<Node>, bool),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat { node: Box<Node>, min: u32, max: Option<u32>, greedy: bool },
}

#[derive(Clone, Debug)]
enum ClassItem {
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

pub(crate) fn is_space(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

fn is_line_end(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

struct Parser<'a> {
    chars: Vec<char>,
    pos: usize,
    groups: usize,
    names: Vec<(String, usize)>,
    _src: &'a str,
}

type Parsed<T> = std::result::Result<T, String>;

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn looking_at(&self, s: &str) -> bool {
        s.chars().enumerate().all(|(i, c)| self.chars.get(self.pos + i) == Some(&c))
    }

    fn disjunction(&mut self) -> Parsed<Node> {
        let mut alts = vec![self.alternative()?];
        while self.eat('|') {
            alts.push(self.alternative()?);
        }
        Ok(if alts.len() == 1 { alts.pop().unwrap() } else { Node::Alt(alts) })
    }

    fn alternative(&mut self) -> Parsed<Node> {
        let mut terms = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            terms.push(self.term()?);
        }
        Ok(match terms.len() {
            0 => Node::Empty,
            1 => terms.pop().unwrap(),
            _ => Node::Concat(terms),
        })
    }

    fn term(&mut self) -> Parsed<Node> {
        let atom = self.atom()?;
        // an assertion takes no quantifier
        if matches!(atom, Node::Start | Node::End | Node::WordBoundary(_)) {
            return Ok(atom);
        }
        self.quantified(atom)
    }

    fn quantified(&mut self, atom: Node) -> Parsed<Node> {
        let (min, max) = match self.peek() {
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('?') => (0, Some(1)),
            Some('{') => match self.braces() {
                Some(q) => q,
                None => return Ok(atom),
            },
            _ => return Ok(atom),
        };
        if !matches!(self.chars.get(self.pos.wrapping_sub(1)), Some('}')) {
            self.pos += 1;
        }
        let greedy = !self.eat('?');
        if max.is_some_and(|m| m < min) {
            return Err("numbers out of order in {} quantifier".into());
        }
        if min > 1000 || max.is_some_and(|m| m > 1000) {
            return Err("quantifier too large".into());
        }
        Ok(Node::Repeat { node: Box::new(atom), min, max, greedy })
    }

    /// `{n}`, `{n,}`, `{n,m}`; anything else in braces is the characters themselves.
    fn braces(&mut self) -> Option<(u32, Option<u32>)> {
        let start = self.pos;
        self.pos += 1;
        let number = |p: &mut Parser| -> Option<u32> {
            let from = p.pos;
            while p.peek().is_some_and(|c| c.is_ascii_digit()) {
                p.pos += 1;
            }
            p.chars[from..p.pos].iter().collect::<String>().parse().ok()
        };
        let parsed = (|| {
            let min = number(self)?;
            let max = if self.eat(',') {
                if self.peek() == Some('}') {
                    None
                } else {
                    Some(number(self)?)
                }
            } else {
                Some(min)
            };
            self.eat('}').then_some((min, max))
        })();
        if parsed.is_none() {
            self.pos = start;
        }
        parsed
    }

    fn atom(&mut self) -> Parsed<Node> {
        let c = self.peek().ok_or("unexpected end")?;
        self.pos += 1;
        match c {
            '^' => Ok(Node::Start),
            '$' => Ok(Node::End),
            '.' => Ok(Node::Any),
            '(' => self.group(),
            '[' => self.class(),
            '\\' => self.escape(),
            '*' | '+' | '?' => Err("nothing to repeat".into()),
            c => Ok(Node::Char(c)),
        }
    }

    fn group(&mut self) -> Parsed<Node> {
        let node = if self.looking_at("?:") {
            self.pos += 2;
            Node::Group(Box::new(self.disjunction()?), None)
        } else if self.looking_at("?=") || self.looking_at("?!") {
            let negate = self.chars[self.pos + 1] == '!';
            self.pos += 2;
            Node::Look(Box::new(self.disjunction()?), negate)
        } else if self.looking_at("?<=") || self.looking_at("?<!") {
            return Err("lookbehind is not supported".into());
        } else if self.looking_at("?<") {
            self.pos += 2;
            let from = self.pos;
            while self.peek().is_some_and(|c| c != '>') {
                self.pos += 1;
            }
            let name: String = self.chars[from..self.pos].iter().collect();
            if !self.eat('>') {
                return Err("invalid capture group name".into());
            }
            self.groups += 1;
            let index = self.groups;
            self.names.push((name, index));
            Node::Group(Box::new(self.disjunction()?), Some(index))
        } else {
            self.groups += 1;
            let index = self.groups;
            Node::Group(Box::new(self.disjunction()?), Some(index))
        };
        if !self.eat(')') {
            return Err("unterminated group".into());
        }
        Ok(node)
    }

    fn hex(&mut self, n: usize) -> Option<char> {
        let digits: String = self.chars.get(self.pos..self.pos + n)?.iter().collect();
        let code = u32::from_str_radix(&digits, 16).ok()?;
        self.pos += n;
        char::from_u32(code)
    }

    /// A character an escape stands for, in a class or out of one.
    fn char_escape(&mut self, c: char) -> char {
        match c {
            't' => '\t',
            'n' => '\n',
            'r' => '\r',
            'f' => '\u{c}',
            'v' => '\u{b}',
            '0' if !self.peek().is_some_and(|d| d.is_ascii_digit()) => '\0',
            'x' => self.hex(2).unwrap_or('x'),
            'u' => {
                if self.peek() == Some('{') {
                    let from = self.pos + 1;
                    let end = self.chars[from..].iter().position(|c| *c == '}').map(|i| from + i);
                    match end.and_then(|end| {
                        let digits: String = self.chars[from..end].iter().collect();
                        Some((end, char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?))
                    }) {
                        Some((end, ch)) => {
                            self.pos = end + 1;
                            ch
                        }
                        None => 'u',
                    }
                } else {
                    self.hex(4).unwrap_or('u')
                }
            }
            'c' => match self.peek() {
                Some(l) if l.is_ascii_alphabetic() => {
                    self.pos += 1;
                    char::from_u32(l as u32 % 32).unwrap_or(l)
                }
                _ => 'c',
            },
            other => other,
        }
    }

    fn escape(&mut self) -> Parsed<Node> {
        let c = self.peek().ok_or("\\ at end of pattern")?;
        self.pos += 1;
        Ok(match c {
            'd' => Node::Class(vec![ClassItem::Digit(true)], false),
            'D' => Node::Class(vec![ClassItem::Digit(false)], false),
            'w' => Node::Class(vec![ClassItem::Word(true)], false),
            'W' => Node::Class(vec![ClassItem::Word(false)], false),
            's' => Node::Class(vec![ClassItem::Space(true)], false),
            'S' => Node::Class(vec![ClassItem::Space(false)], false),
            'b' => Node::WordBoundary(true),
            'B' => Node::WordBoundary(false),
            'k' if self.peek() == Some('<') => {
                let from = self.pos + 1;
                let end = self.chars[from..]
                    .iter()
                    .position(|c| *c == '>')
                    .map(|i| from + i)
                    .ok_or("invalid named reference")?;
                let name: String = self.chars[from..end].iter().collect();
                self.pos = end + 1;
                let index =
                    self.names.iter().find(|(n, _)| *n == name).map(|(_, i)| *i).ok_or("invalid named reference")?;
                Node::Backref(index)
            }
            '1'..='9' => {
                let from = self.pos - 1;
                while self.peek().is_some_and(|d| d.is_ascii_digit()) {
                    self.pos += 1;
                }
                let n: usize =
                    self.chars[from..self.pos].iter().collect::<String>().parse().map_err(|_| "invalid reference")?;
                Node::Backref(n)
            }
            other => Node::Char(self.char_escape(other)),
        })
    }

    fn class(&mut self) -> Parsed<Node> {
        let negated = self.eat('^');
        let mut items = Vec::new();
        loop {
            let c = self.peek().ok_or("unterminated character class")?;
            self.pos += 1;
            if c == ']' {
                break;
            }
            let low = if c == '\\' {
                let e = self.peek().ok_or("\\ at end of pattern")?;
                self.pos += 1;
                match e {
                    'd' => {
                        items.push(ClassItem::Digit(true));
                        continue;
                    }
                    'D' => {
                        items.push(ClassItem::Digit(false));
                        continue;
                    }
                    'w' => {
                        items.push(ClassItem::Word(true));
                        continue;
                    }
                    'W' => {
                        items.push(ClassItem::Word(false));
                        continue;
                    }
                    's' => {
                        items.push(ClassItem::Space(true));
                        continue;
                    }
                    'S' => {
                        items.push(ClassItem::Space(false));
                        continue;
                    }
                    'b' => '\u{8}',
                    other => self.char_escape(other),
                }
            } else {
                c
            };
            // a range, unless the dash is the class's last character
            if self.peek() == Some('-') && self.chars.get(self.pos + 1).is_some_and(|n| *n != ']') {
                self.pos += 1;
                let mut high = self.peek().ok_or("unterminated character class")?;
                self.pos += 1;
                if high == '\\' {
                    let e = self.peek().ok_or("\\ at end of pattern")?;
                    self.pos += 1;
                    if matches!(e, 'd' | 'D' | 'w' | 'W' | 's' | 'S') {
                        // a class is no end of a range: the dash is itself
                        items.push(ClassItem::Range(low, low));
                        items.push(ClassItem::Range('-', '-'));
                        self.pos -= 2;
                        continue;
                    }
                    high = if e == 'b' { '\u{8}' } else { self.char_escape(e) };
                }
                if high < low {
                    return Err("range out of order in character class".into());
                }
                items.push(ClassItem::Range(low, high));
            } else {
                items.push(ClassItem::Range(low, low));
            }
        }
        Ok(Node::Class(items, negated))
    }
}

// ---------------------------------------------------------------- program

#[derive(Debug)]
enum Inst {
    Char(char),
    Any,
    Class(Vec<ClassItem>, bool),
    Start,
    End,
    WordBoundary(bool),
    Save(usize),
    /// Try the first; on failure the second
    Split(usize, usize),
    Jmp(usize),
    Backref(usize),
    /// A lookahead over the instructions up to `end`; the program goes on from there
    Look {
        negate: bool,
        end: usize,
    },
    /// Where an iteration of a loop began
    Mark(usize),
    /// An iteration that matched nothing is no iteration
    Progress(usize),
    Match,
}

pub struct Program {
    insts: Vec<Inst>,
    groups: usize,
    marks: usize,
    ignore_case: bool,
    multiline: bool,
    dot_all: bool,
}

fn compile(source: &str, flags: &str) -> std::result::Result<Program, String> {
    let mut parser = Parser { chars: source.chars().collect(), pos: 0, groups: 0, names: Vec::new(), _src: source };
    let node = parser.disjunction()?;
    if parser.pos < parser.chars.len() {
        return Err("unmatched ')'".into());
    }
    let mut c = Compiler { insts: Vec::new(), marks: 0 };
    c.emit(Inst::Save(0));
    c.node(&node)?;
    c.emit(Inst::Save(1));
    c.emit(Inst::Match);
    Ok(Program {
        insts: c.insts,
        groups: parser.groups + 1,
        marks: c.marks,
        ignore_case: flags.contains('i'),
        multiline: flags.contains('m'),
        dot_all: flags.contains('s'),
    })
}

struct Compiler {
    insts: Vec<Inst>,
    marks: usize,
}

impl Compiler {
    fn emit(&mut self, inst: Inst) -> usize {
        self.insts.push(inst);
        self.insts.len() - 1
    }

    fn node(&mut self, node: &Node) -> std::result::Result<(), String> {
        if self.insts.len() > 100_000 {
            return Err("regular expression too large".into());
        }
        match node {
            Node::Empty => {}
            Node::Char(c) => {
                self.emit(Inst::Char(*c));
            }
            Node::Any => {
                self.emit(Inst::Any);
            }
            Node::Class(items, negated) => {
                self.emit(Inst::Class(items.clone(), *negated));
            }
            Node::Start => {
                self.emit(Inst::Start);
            }
            Node::End => {
                self.emit(Inst::End);
            }
            Node::WordBoundary(b) => {
                self.emit(Inst::WordBoundary(*b));
            }
            Node::Group(inner, None) => self.node(inner)?,
            Node::Group(inner, Some(i)) => {
                self.emit(Inst::Save(2 * i));
                self.node(inner)?;
                self.emit(Inst::Save(2 * i + 1));
            }
            Node::Backref(i) => {
                self.emit(Inst::Backref(*i));
            }
            Node::Look(inner, negate) => {
                let look = self.emit(Inst::Look { negate: *negate, end: 0 });
                self.node(inner)?;
                self.emit(Inst::Match);
                let end = self.insts.len();
                self.insts[look] = Inst::Look { negate: *negate, end };
            }
            Node::Concat(nodes) => {
                for n in nodes {
                    self.node(n)?;
                }
            }
            Node::Alt(alts) => {
                let mut jumps = Vec::new();
                for (i, alt) in alts.iter().enumerate() {
                    if i + 1 < alts.len() {
                        let split = self.emit(Inst::Split(0, 0));
                        self.node(alt)?;
                        jumps.push(self.emit(Inst::Jmp(0)));
                        let next = self.insts.len();
                        self.insts[split] = Inst::Split(split + 1, next);
                    } else {
                        self.node(alt)?;
                    }
                }
                let end = self.insts.len();
                for j in jumps {
                    self.insts[j] = Inst::Jmp(end);
                }
            }
            Node::Repeat { node, min, max, greedy } => {
                for _ in 0..*min {
                    self.node(node)?;
                }
                match max {
                    None => {
                        // L1: split L2, L3; L2: mark; node; progress; jmp L1; L3:
                        let mark = self.marks;
                        self.marks += 1;
                        let split = self.emit(Inst::Split(0, 0));
                        self.emit(Inst::Mark(mark));
                        self.node(node)?;
                        self.emit(Inst::Progress(mark));
                        self.emit(Inst::Jmp(split));
                        let end = self.insts.len();
                        self.insts[split] =
                            if *greedy { Inst::Split(split + 1, end) } else { Inst::Split(end, split + 1) };
                    }
                    Some(max) => {
                        // each further copy is optional, and inside the one before it
                        let mut splits = Vec::new();
                        for _ in *min..*max {
                            splits.push(self.emit(Inst::Split(0, 0)));
                            self.node(node)?;
                        }
                        let end = self.insts.len();
                        for split in splits {
                            self.insts[split] =
                                if *greedy { Inst::Split(split + 1, end) } else { Inst::Split(end, split + 1) };
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- matching

enum Undo {
    Branch(usize, usize),
    Cap(usize, Option<usize>),
    Mark(usize, usize),
}

const STEP_LIMIT: usize = 20_000_000;

impl Program {
    fn fold(&self, c: char) -> char {
        if self.ignore_case {
            let mut upper = c.to_uppercase();
            match (upper.next(), upper.next()) {
                (Some(u), None) => u,
                _ => c,
            }
        } else {
            c
        }
    }

    fn class_matches(&self, items: &[ClassItem], negated: bool, c: char) -> bool {
        let folded = self.fold(c);
        let hit = items.iter().any(|item| match item {
            ClassItem::Range(lo, hi) => {
                (*lo <= c && c <= *hi)
                    || (self.ignore_case && {
                        let lower = c.to_lowercase().next().unwrap_or(c);
                        (*lo <= folded && folded <= *hi) || (*lo <= lower && lower <= *hi)
                    })
            }
            ClassItem::Digit(yes) => c.is_ascii_digit() == *yes,
            ClassItem::Word(yes) => is_word(c) == *yes,
            ClassItem::Space(yes) => is_space(c) == *yes,
        });
        hit != negated
    }

    /// Runs the program from `pc` at `pos`; the captures of the match it finds.
    fn run(
        &self,
        input: &[char],
        start_pc: usize,
        start: usize,
        caps: &mut [Option<usize>],
        steps: &mut usize,
    ) -> Result<bool> {
        let mut marks = vec![usize::MAX; self.marks];
        let mut stack: Vec<Undo> = Vec::new();
        let (mut pc, mut pos) = (start_pc, start);
        loop {
            *steps += 1;
            if *steps > STEP_LIMIT {
                return Err(Error::msg("Regular expression took too long to match"));
            }
            let ok = match &self.insts[pc] {
                Inst::Match => return Ok(true),
                Inst::Char(c) => {
                    if pos < input.len()
                        && (input[pos] == *c || (self.ignore_case && self.fold(input[pos]) == self.fold(*c)))
                    {
                        pos += 1;
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::Any => {
                    if pos < input.len() && (self.dot_all || !is_line_end(input[pos])) {
                        pos += 1;
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::Class(items, negated) => {
                    if pos < input.len() && self.class_matches(items, *negated, input[pos]) {
                        pos += 1;
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::Start => {
                    let at = pos == 0 || (self.multiline && is_line_end(input[pos - 1]));
                    pc += 1;
                    at
                }
                Inst::End => {
                    let at = pos == input.len() || (self.multiline && is_line_end(input[pos]));
                    pc += 1;
                    at
                }
                Inst::WordBoundary(want) => {
                    let before = pos > 0 && is_word(input[pos - 1]);
                    let after = pos < input.len() && is_word(input[pos]);
                    pc += 1;
                    (before != after) == *want
                }
                Inst::Save(i) => {
                    stack.push(Undo::Cap(*i, caps[*i]));
                    caps[*i] = Some(pos);
                    pc += 1;
                    true
                }
                Inst::Split(first, second) => {
                    stack.push(Undo::Branch(*second, pos));
                    pc = *first;
                    true
                }
                Inst::Jmp(to) => {
                    pc = *to;
                    true
                }
                Inst::Backref(i) => {
                    // a group that has not matched is the empty string
                    let (from, to) = match (caps.get(2 * i).copied().flatten(), caps.get(2 * i + 1).copied().flatten())
                    {
                        (Some(a), Some(b)) if a <= b => (a, b),
                        _ => (0, 0),
                    };
                    let len = to - from;
                    let same = pos + len <= input.len()
                        && (0..len).all(|k| {
                            input[from + k] == input[pos + k]
                                || (self.ignore_case && self.fold(input[from + k]) == self.fold(input[pos + k]))
                        });
                    if same {
                        pos += len;
                        pc += 1;
                    }
                    same
                }
                Inst::Look { negate, end } => {
                    let mut inner = caps.to_vec();
                    let found = self.run(input, pc + 1, pos, &mut inner, steps)?;
                    if found && !*negate {
                        for (i, (old, new)) in caps.iter_mut().zip(inner).enumerate() {
                            if *old != new {
                                stack.push(Undo::Cap(i, *old));
                                *old = new;
                            }
                        }
                    }
                    pc = *end;
                    found != *negate
                }
                Inst::Mark(r) => {
                    stack.push(Undo::Mark(*r, marks[*r]));
                    marks[*r] = pos;
                    pc += 1;
                    true
                }
                Inst::Progress(r) => {
                    pc += 1;
                    marks[*r] != pos
                }
            };
            if ok {
                continue;
            }
            // back to the last branch not yet taken
            loop {
                match stack.pop() {
                    None => return Ok(false),
                    Some(Undo::Cap(i, old)) => caps[i] = old,
                    Some(Undo::Mark(r, old)) => marks[r] = old,
                    Some(Undo::Branch(to, at)) => {
                        pc = to;
                        pos = at;
                        break;
                    }
                }
            }
        }
    }

    fn exec(&self, input: &str) -> Result<Option<Match>> {
        let chars: Vec<char> = input.chars().collect();
        let mut steps = 0usize;
        for start in 0..=chars.len() {
            let mut caps = vec![None; self.groups * 2];
            if self.run(&chars, 0, start, &mut caps, &mut steps)? {
                let groups = (0..self.groups)
                    .map(|g| match (caps[2 * g], caps[2 * g + 1]) {
                        (Some(a), Some(b)) if a <= b => Some(chars[a..b].iter().collect::<String>()),
                        _ => None,
                    })
                    .collect();
                let index = chars[..start].iter().map(|c| c.len_utf16()).sum();
                return Ok(Some(Match { index, groups }));
            }
        }
        Ok(None)
    }
}

/// A compiled program, or why the source is not one. Kept by the regular expression it was compiled for.
pub type Compiled = std::result::Result<Arc<Program>, String>;

#[cfg(test)]
mod tests {
    use super::*;

    fn find(source: &str, flags: &str, input: &str) -> Option<Vec<Option<String>>> {
        exec(&Regex::new(source, flags), input).unwrap().map(|m| m.groups)
    }

    fn whole(source: &str, input: &str) -> Option<String> {
        find(source, "", input).and_then(|g| g[0].clone())
    }

    #[test]
    fn matches_as_javascript_does() {
        assert_eq!(whole("a+", "caaat").as_deref(), Some("aaa"));
        assert_eq!(whole("a+?", "caaat").as_deref(), Some("a"));
        assert_eq!(whole("^\\d{2,3}$", "1234"), None);
        assert_eq!(whole("^\\d{2,3}$", "123").as_deref(), Some("123"));
        assert_eq!(whole("colou?r", "my color").as_deref(), Some("color"));
        assert_eq!(whole("[^a-c]+", "abcxyzabc").as_deref(), Some("xyz"));
        assert_eq!(whole("\\bfoo\\b", "a foo b").as_deref(), Some("foo"));
        assert_eq!(whole("\\bfoo\\b", "afoo b"), None);
        assert_eq!(whole("(?:ab)*c", "ababc").as_deref(), Some("ababc"));
        assert_eq!(whole("a|ab", "ab").as_deref(), Some("a"));
        assert_eq!(whole("(a*)*b", "aaab").as_deref(), Some("aaab"));
        assert_eq!(whole("x*", "abc").as_deref(), Some(""));
        assert_eq!(whole("foo(?=bar)", "foobar").as_deref(), Some("foo"));
        assert_eq!(whole("foo(?!bar)", "foobar"), None);
        assert_eq!(whole("(\\w)\\1", "hello").as_deref(), Some("ll"));
        assert_eq!(whole("a.c", "a\nc"), None);
        assert_eq!(whole("[a-]", "-").as_deref(), Some("-"));
        assert_eq!(whole("a{,2}", "a{,2}").as_deref(), Some("a{,2}"));
        assert_eq!(whole("\\u0041\\x42", "AB").as_deref(), Some("AB"));
    }

    #[test]
    fn groups_and_flags() {
        assert_eq!(
            find("(\\d+)-(\\d+)?(x)?", "", "ab 12-34"),
            Some(vec![Some("12-34".into()), Some("12".into()), Some("34".into()), None])
        );
        assert_eq!(find("HELLO", "i", "say hello").map(|g| g[0].clone()), Some(Some("hello".into())));
        assert_eq!(find("^b", "m", "a\nb").map(|g| g[0].clone()), Some(Some("b".into())));
        assert_eq!(find("a.c", "s", "a\nc").map(|g| g[0].clone()), Some(Some("a\nc".into())));
        assert_eq!(find("(?<y>\\d{4})-\\k<y>", "", "2020-2020").map(|g| g.len()), Some(2));
        assert!(exec(&Regex::new("(", ""), "x").is_err());
        assert!(exec(&Regex::new("a**", ""), "x").is_err());
        let m = exec(&Regex::new("c", ""), "ab😀c").unwrap().unwrap();
        assert_eq!(m.index, 4);
    }
}
