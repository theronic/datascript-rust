//! A persistent sorted set: a B-tree whose nodes are shared between the values made from one another, as
//! `me.tonsky.persistent-sorted-set` is, which DataScript keeps its indexes in. The order is not the set's own: each
//! operation is given its comparator, as DataScript gives `conj`, `disj` and `slice` theirs.
//!
//! A set that nothing else shares is changed in place, which is what DataScript's transient indexes are for.

use std::cmp::Ordering;
use std::sync::Arc;

/// The most elements a node holds; a node splits beyond it, and is merged or refilled under half of it.
const MAX_LEN: usize = 32;
const MIN_LEN: usize = MAX_LEN / 2;
/// Leaves are built this full by `from_sorted`
const AVG_LEN: usize = (MAX_LEN + MIN_LEN) / 2;
const MAX_DEPTH: usize = 12;

#[derive(Clone)]
enum Node<T> {
    Leaf(Vec<T>),
    Branch {
        /// The greatest element under each child
        keys: Vec<T>,
        children: Vec<Arc<Node<T>>>,
    },
}

impl<T: Clone> Node<T> {
    fn len(&self) -> usize {
        match self {
            Node::Leaf(items) => items.len(),
            Node::Branch { children, .. } => children.len(),
        }
    }

    fn max(&self) -> Option<&T> {
        match self {
            Node::Leaf(items) => items.last(),
            Node::Branch { keys, .. } => keys.last(),
        }
    }
}

pub struct SortedSet<T> {
    root: Arc<Node<T>>,
    len: usize,
    /// Levels of branches above the leaves
    depth: usize,
}

impl<T> Clone for SortedSet<T> {
    fn clone(&self) -> Self {
        SortedSet { root: self.root.clone(), len: self.len, depth: self.depth }
    }
}

impl<T: Clone> Default for SortedSet<T> {
    fn default() -> Self {
        SortedSet::new()
    }
}

/// A place in a set: the child taken at each level from the root, and the index in the leaf. `end` is the place
/// after the last element.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pos {
    path: [u16; MAX_DEPTH],
}

impl PartialOrd for Pos {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Pos {
    fn cmp(&self, other: &Self) -> Ordering {
        self.path.cmp(&other.path)
    }
}

enum Inserted<T> {
    No,
    Yes,
    Split(Arc<Node<T>>),
}

impl<T: Clone> SortedSet<T> {
    pub fn new() -> SortedSet<T> {
        SortedSet { root: Arc::new(Node::Leaf(Vec::new())), len: 0, depth: 0 }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether both are the same tree, so equal without looking.
    pub fn ptr_eq(&self, other: &SortedSet<T>) -> bool {
        Arc::ptr_eq(&self.root, &other.root)
    }

    /// A set of elements already in order and distinct.
    pub fn from_sorted(items: Vec<T>) -> SortedSet<T> {
        let len = items.len();
        if len == 0 {
            return SortedSet::new();
        }
        let mut level: Vec<Arc<Node<T>>> = partition(items).into_iter().map(|c| Arc::new(Node::Leaf(c))).collect();
        let mut depth = 0;
        while level.len() > 1 {
            level = partition(level)
                .into_iter()
                .map(|children| {
                    let keys = children.iter().map(|c| c.max().expect("a node is not empty").clone()).collect();
                    Arc::new(Node::Branch { keys, children })
                })
                .collect();
            depth += 1;
        }
        SortedSet { root: level.pop().expect("a root"), len, depth }
    }

    /// `conj` with a comparator. False when an element that compares equal is already there.
    pub fn insert(&mut self, x: T, cmp: &impl Fn(&T, &T) -> Ordering) -> bool {
        match insert(&mut self.root, x, cmp) {
            Inserted::No => false,
            Inserted::Yes => {
                self.len += 1;
                true
            }
            Inserted::Split(right) => {
                let left = self.root.clone();
                let keys = vec![left.max().expect("left").clone(), right.max().expect("right").clone()];
                self.root = Arc::new(Node::Branch { keys, children: vec![left, right] });
                self.depth += 1;
                self.len += 1;
                true
            }
        }
    }

    /// `disj` with a comparator. False when no element compares equal.
    pub fn remove(&mut self, x: &T, cmp: &impl Fn(&T, &T) -> Ordering) -> bool {
        if !remove(&mut self.root, x, cmp) {
            return false;
        }
        self.len -= 1;
        // a root with one child gives way to it
        loop {
            let only = match &*self.root {
                Node::Branch { children, .. } if children.len() == 1 => children[0].clone(),
                _ => break,
            };
            self.root = only;
            self.depth -= 1;
        }
        true
    }

    /// The place of the first element.
    pub fn start(&self) -> Pos {
        Pos { path: [0; MAX_DEPTH] }
    }

    /// The place after the last element.
    pub fn end(&self) -> Pos {
        let mut path = [0u16; MAX_DEPTH];
        let mut node = &*self.root;
        let mut level = 0;
        loop {
            match node {
                Node::Leaf(items) => {
                    path[level] = items.len() as u16;
                    return Pos { path };
                }
                Node::Branch { children, .. } => {
                    path[level] = (children.len() - 1) as u16;
                    node = &children[children.len() - 1];
                    level += 1;
                }
            }
        }
    }

    /// The place of the first element that is not less than a bound: `f` orders an element against the bound.
    pub fn lower_bound(&self, f: impl Fn(&T) -> Ordering) -> Pos {
        self.bound(|x| f(x) != Ordering::Less)
    }

    /// The place of the first element greater than a bound.
    pub fn upper_bound(&self, f: impl Fn(&T) -> Ordering) -> Pos {
        self.bound(|x| f(x) == Ordering::Greater)
    }

    /// The place of the first element for which `reached` holds, in a set where it holds from some element on.
    fn bound(&self, reached: impl Fn(&T) -> bool) -> Pos {
        let mut path = [0u16; MAX_DEPTH];
        let mut node = &*self.root;
        let mut level = 0;
        loop {
            match node {
                Node::Leaf(items) => {
                    let i = items.partition_point(|x| !reached(x));
                    path[level] = i as u16;
                    let pos = Pos { path };
                    return if i == items.len() { self.normalize(pos) } else { pos };
                }
                Node::Branch { keys, children } => {
                    let i = keys.partition_point(|x| !reached(x));
                    if i == children.len() {
                        return self.end();
                    }
                    path[level] = i as u16;
                    node = &children[i];
                    level += 1;
                }
            }
        }
    }

    /// A place past the end of its leaf is the start of the next leaf, or the set's end.
    fn normalize(&self, pos: Pos) -> Pos {
        self.next_leaf(&pos).unwrap_or_else(|| self.end())
    }

    /// The place of the first element of the leaf after this place's, if there is one.
    fn next_leaf(&self, pos: &Pos) -> Option<Pos> {
        let mut path = pos.path;
        let mut nodes: [Option<&Node<T>>; MAX_DEPTH] = [None; MAX_DEPTH];
        let mut node = &*self.root;
        for level in 0..self.depth {
            nodes[level] = Some(node);
            if let Node::Branch { children, .. } = node {
                node = &children[path[level] as usize];
            }
        }
        // the deepest level that has a next child
        for level in (0..self.depth).rev() {
            if (path[level] as usize) + 1 < nodes[level].map_or(0, Node::len) {
                path[level] += 1;
                for p in path.iter_mut().skip(level + 1) {
                    *p = 0;
                }
                return Some(Pos { path });
            }
        }
        None
    }

    /// The elements from one place up to another.
    pub fn slice(&self, from: Pos, to: Pos) -> Slice<T> {
        Slice { set: self.clone(), from, to }
    }

    /// The whole set.
    pub fn all(&self) -> Slice<T> {
        self.slice(self.start(), self.end())
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter::new(self, self.start(), self.end())
    }

    /// The element at a place, when there is one.
    pub fn at(&self, pos: Pos) -> Option<&T> {
        let mut node = &*self.root;
        let mut level = 0;
        loop {
            match node {
                Node::Leaf(items) => return items.get(pos.path[level] as usize),
                Node::Branch { children, .. } => {
                    node = children.get(pos.path[level] as usize)?;
                    level += 1;
                }
            }
        }
    }

    fn leaf(&self, pos: &Pos) -> &[T] {
        let mut node = &*self.root;
        let mut level = 0;
        loop {
            match node {
                Node::Leaf(items) => return items,
                Node::Branch { children, .. } => {
                    node = &children[pos.path[level] as usize];
                    level += 1;
                }
            }
        }
    }

    /// The place of the last element of the leaf before this one's, if any.
    fn prev_leaf_end(&self, pos: &Pos) -> Option<Pos> {
        let mut path = pos.path;
        for level in (0..self.depth).rev() {
            if path[level] > 0 {
                path[level] -= 1;
                // down the last child at each level below
                let mut node = &*self.root;
                for p in path.iter().take(level + 1) {
                    if let Node::Branch { children, .. } = node {
                        node = &children[*p as usize];
                    }
                }
                for p in path.iter_mut().take(self.depth).skip(level + 1) {
                    match node {
                        Node::Branch { children, .. } => {
                            *p = (children.len() - 1) as u16;
                            node = &children[children.len() - 1];
                        }
                        Node::Leaf(_) => break,
                    }
                }
                path[self.depth] = node.len() as u16;
                return Some(Pos { path });
            }
        }
        None
    }
}

/// Splits into runs between `MIN_LEN` and `MAX_LEN` long, near `AVG_LEN` (`arr-partition-approx`).
fn partition<X>(items: Vec<X>) -> Vec<Vec<X>> {
    let len = items.len();
    let mut out = Vec::with_capacity(len / AVG_LEN + 1);
    let mut it = items.into_iter();
    let mut pos = 0;
    while pos < len {
        let rest = len - pos;
        let take = if rest <= MAX_LEN {
            rest
        } else if rest >= AVG_LEN + MIN_LEN {
            AVG_LEN
        } else {
            rest / 2
        };
        out.push(it.by_ref().take(take).collect());
        pos += take;
    }
    out
}

fn insert<T: Clone>(node: &mut Arc<Node<T>>, x: T, cmp: &impl Fn(&T, &T) -> Ordering) -> Inserted<T> {
    // look before making the node this set's own, so that an element already there copies nothing
    match &**node {
        Node::Leaf(items) => {
            let i = items.partition_point(|e| cmp(e, &x) == Ordering::Less);
            if i < items.len() && cmp(&items[i], &x) == Ordering::Equal {
                return Inserted::No;
            }
            let Node::Leaf(items) = Arc::make_mut(node) else { unreachable!() };
            items.insert(i, x);
            if items.len() > MAX_LEN {
                let right = items.split_off(items.len() / 2);
                Inserted::Split(Arc::new(Node::Leaf(right)))
            } else {
                Inserted::Yes
            }
        }
        Node::Branch { keys, .. } => {
            let mut i = keys.partition_point(|e| cmp(e, &x) == Ordering::Less);
            if i < keys.len() && cmp(&keys[i], &x) == Ordering::Equal {
                return Inserted::No;
            }
            if i == keys.len() {
                i -= 1;
            }
            let Node::Branch { keys, children } = Arc::make_mut(node) else { unreachable!() };
            match insert(&mut children[i], x, cmp) {
                Inserted::No => Inserted::No,
                Inserted::Yes => {
                    keys[i] = children[i].max().expect("child").clone();
                    Inserted::Yes
                }
                Inserted::Split(right) => {
                    keys[i] = children[i].max().expect("child").clone();
                    keys.insert(i + 1, right.max().expect("right").clone());
                    children.insert(i + 1, right);
                    if children.len() > MAX_LEN {
                        let at = children.len() / 2;
                        let right_children = children.split_off(at);
                        let right_keys = keys.split_off(at);
                        Inserted::Split(Arc::new(Node::Branch { keys: right_keys, children: right_children }))
                    } else {
                        Inserted::Yes
                    }
                }
            }
        }
    }
}

fn remove<T: Clone>(node: &mut Arc<Node<T>>, x: &T, cmp: &impl Fn(&T, &T) -> Ordering) -> bool {
    match &**node {
        Node::Leaf(items) => {
            let i = items.partition_point(|e| cmp(e, x) == Ordering::Less);
            if i >= items.len() || cmp(&items[i], x) != Ordering::Equal {
                return false;
            }
            let Node::Leaf(items) = Arc::make_mut(node) else { unreachable!() };
            items.remove(i);
            true
        }
        Node::Branch { keys, .. } => {
            let i = keys.partition_point(|e| cmp(e, x) == Ordering::Less);
            if i == keys.len() {
                return false;
            }
            let Node::Branch { keys, children } = Arc::make_mut(node) else { unreachable!() };
            if !remove(&mut children[i], x, cmp) {
                return false;
            }
            if children[i].len() == 0 {
                children.remove(i);
                keys.remove(i);
                return true;
            }
            keys[i] = children[i].max().expect("child").clone();
            if children[i].len() < MIN_LEN && children.len() > 1 {
                // with the neighbour on the left, or on the right of the first
                let (l, r) = if i > 0 { (i - 1, i) } else { (i, i + 1) };
                rebalance(keys, children, l, r);
            }
            true
        }
    }
}

/// Joins two neighbours when one node holds them, or shares their elements evenly between them.
fn rebalance<T: Clone>(keys: &mut Vec<T>, children: &mut Vec<Arc<Node<T>>>, l: usize, r: usize) {
    let total = children[l].len() + children[r].len();
    let right = children.remove(r);
    keys.remove(r);
    let right = Arc::try_unwrap(right).unwrap_or_else(|shared| (*shared).clone());
    let left = Arc::make_mut(&mut children[l]);
    match (left, right) {
        (Node::Leaf(a), Node::Leaf(mut b)) => {
            a.append(&mut b);
            if total > MAX_LEN {
                let b = a.split_off(total / 2);
                keys.insert(r, b.last().expect("right").clone());
                children.insert(r, Arc::new(Node::Leaf(b)));
            }
        }
        (Node::Branch { keys: ak, children: ac }, Node::Branch { keys: mut bk, children: mut bc }) => {
            ak.append(&mut bk);
            ac.append(&mut bc);
            if total > MAX_LEN {
                let bc = ac.split_off(total / 2);
                let bk = ak.split_off(total / 2);
                keys.insert(r, bk.last().expect("right").clone());
                children.insert(r, Arc::new(Node::Branch { keys: bk, children: bc }));
            }
        }
        _ => unreachable!("neighbours are of one level"),
    }
    keys[l] = children[l].max().expect("left").clone();
}

/// A run of a set's elements, from one place up to another. It holds the set, so it outlives the database value it
/// was taken from.
pub struct Slice<T> {
    set: SortedSet<T>,
    from: Pos,
    to: Pos,
}

impl<T> Clone for Slice<T> {
    fn clone(&self) -> Self {
        Slice { set: self.set.clone(), from: self.from, to: self.to }
    }
}

impl<T: Clone> Slice<T> {
    pub fn empty() -> Slice<T> {
        let set = SortedSet::new();
        let pos = set.start();
        Slice { set, from: pos, to: pos }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.from >= self.to
    }

    pub fn first(&self) -> Option<&T> {
        if self.is_empty() {
            None
        } else {
            self.set.at(self.from)
        }
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter::new(&self.set, self.from, self.to)
    }

    /// From the last element back to the first.
    pub fn iter_rev(&self) -> RevIter<'_, T> {
        RevIter::new(&self.set, self.from, self.to)
    }

    /// The place of the run's first element, and the place after its last.
    pub fn bounds(&self) -> (Pos, Pos) {
        (self.from, self.to)
    }

    /// The run from a place in it on: where an iteration that stopped goes on.
    pub fn iter_from(&self, from: Pos) -> Iter<'_, T> {
        Iter::new(&self.set, from.max(self.from), self.to)
    }

    /// The run backwards from before a place in it.
    pub fn iter_rev_from(&self, to: Pos) -> RevIter<'_, T> {
        RevIter::new(&self.set, self.from, to.min(self.to))
    }

    pub fn to_vec(&self) -> Vec<T> {
        self.iter().cloned().collect()
    }

    pub fn count(&self) -> usize {
        if self.is_empty() {
            return 0;
        }
        if self.from == self.set.start() && self.to == self.set.end() {
            return self.set.len();
        }
        self.iter().count()
    }
}

/// Elements in order, a leaf at a time.
pub struct Iter<'a, T> {
    set: &'a SortedSet<T>,
    pos: Pos,
    to: Pos,
    leaf: &'a [T],
    /// Where this leaf's part of the run ends
    leaf_end: usize,
}

impl<'a, T: Clone> Iter<'a, T> {
    fn new(set: &'a SortedSet<T>, from: Pos, to: Pos) -> Iter<'a, T> {
        let mut it = Iter { set, pos: from, to, leaf: &[], leaf_end: 0 };
        if from < to {
            it.enter_leaf();
        }
        it
    }

    fn enter_leaf(&mut self) {
        let depth = self.set.depth;
        self.leaf = self.set.leaf(&self.pos);
        self.leaf_end = if self.pos.path[..depth] == self.to.path[..depth] {
            (self.to.path[depth] as usize).min(self.leaf.len())
        } else {
            self.leaf.len()
        };
    }
}

impl<T> Iter<'_, T> {
    /// The place of the element `next` would give.
    pub fn pos(&self) -> Pos {
        self.pos
    }
}

impl<T> RevIter<'_, T> {
    /// The place after the element `next` would give.
    pub fn pos(&self) -> Pos {
        if self.done {
            self.from
        } else {
            self.pos
        }
    }
}

impl<'a, T: Clone> Iterator for Iter<'a, T> {
    type Item = &'a T;

    #[inline]
    fn next(&mut self) -> Option<&'a T> {
        let depth = self.set.depth;
        loop {
            let i = self.pos.path[depth] as usize;
            if i < self.leaf_end {
                self.pos.path[depth] += 1;
                return Some(&self.leaf[i]);
            }
            if self.pos >= self.to {
                return None;
            }
            // the next leaf
            match self.set.next_leaf(&self.pos) {
                Some(p) if p < self.to => {
                    self.pos = p;
                    self.enter_leaf();
                }
                _ => {
                    self.pos = self.to;
                    self.leaf = &[];
                    self.leaf_end = 0;
                    return None;
                }
            }
        }
    }
}

/// Elements from the last back to the first.
pub struct RevIter<'a, T> {
    set: &'a SortedSet<T>,
    from: Pos,
    /// The place after the next element to give
    pos: Pos,
    leaf: &'a [T],
    /// Where this leaf's part of the run starts
    leaf_start: usize,
    done: bool,
}

impl<'a, T: Clone> RevIter<'a, T> {
    fn new(set: &'a SortedSet<T>, from: Pos, to: Pos) -> RevIter<'a, T> {
        let mut it = RevIter { set, from, pos: to, leaf: &[], leaf_start: 0, done: from >= to };
        if !it.done {
            it.enter_leaf();
        }
        it
    }

    fn enter_leaf(&mut self) {
        let depth = self.set.depth;
        self.leaf = self.set.leaf(&self.pos);
        self.leaf_start =
            if self.pos.path[..depth] == self.from.path[..depth] { self.from.path[depth] as usize } else { 0 };
    }
}

impl<'a, T: Clone> Iterator for RevIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        if self.done {
            return None;
        }
        let depth = self.set.depth;
        loop {
            let i = self.pos.path[depth] as usize;
            if i > self.leaf_start {
                self.pos.path[depth] -= 1;
                return Some(&self.leaf[i - 1]);
            }
            if self.pos <= self.from {
                self.done = true;
                return None;
            }
            match self.set.prev_leaf_end(&self.pos) {
                Some(p) if p > self.from => {
                    self.pos = p;
                    self.enter_leaf();
                }
                _ => {
                    self.done = true;
                    return None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmp(a: &i64, b: &i64) -> Ordering {
        a.cmp(b)
    }

    /// The same sequence every run
    fn shuffled(n: i64) -> Vec<i64> {
        let mut xs: Vec<i64> = (0..n).collect();
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for i in (1..xs.len()).rev() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            xs.swap(i, (state % (i as u64 + 1)) as usize);
        }
        xs
    }

    #[test]
    fn inserts_in_order_whatever_the_order_of_inserting() {
        for n in [0, 1, 31, 32, 33, 1000, 20_000] {
            let mut s = SortedSet::new();
            for x in shuffled(n) {
                assert!(s.insert(x, &cmp));
            }
            assert_eq!(s.len(), n as usize);
            assert_eq!(s.iter().cloned().collect::<Vec<_>>(), (0..n).collect::<Vec<_>>());
            assert!(!s.insert(0, &cmp) || n == 0);
        }
    }

    #[test]
    fn removes() {
        let n = 5000;
        let mut s = SortedSet::new();
        for x in shuffled(n) {
            s.insert(x, &cmp);
        }
        let kept = s.clone();
        for x in shuffled(n).into_iter().filter(|x| x % 3 != 0) {
            assert!(s.remove(&x, &cmp), "{x}");
            assert!(!s.remove(&x, &cmp));
        }
        assert_eq!(s.iter().cloned().collect::<Vec<_>>(), (0..n).filter(|x| x % 3 == 0).collect::<Vec<_>>());
        assert_eq!(s.len(), s.iter().count());
        // the value it was made from is untouched
        assert_eq!(kept.len(), n as usize);
        assert_eq!(kept.iter().cloned().collect::<Vec<_>>(), (0..n).collect::<Vec<_>>());
        for x in (0..n).filter(|x| x % 3 == 0) {
            assert!(s.remove(&x, &cmp));
        }
        assert!(s.is_empty());
        assert_eq!(s.iter().count(), 0);
        assert!(s.insert(7, &cmp));
        assert_eq!(s.iter().cloned().collect::<Vec<_>>(), vec![7]);
    }

    #[test]
    fn from_sorted_and_slices() {
        for n in [0i64, 1, 5, 32, 33, 100, 777, 10_000] {
            let s = SortedSet::from_sorted((0..n).map(|x| x * 2).collect());
            assert_eq!(s.len(), n as usize);
            assert_eq!(s.iter().cloned().collect::<Vec<_>>(), (0..n).map(|x| x * 2).collect::<Vec<_>>());
            for (lo, hi) in
                [(0, 0), (3, 3), (4, 4), (5, 40), (-10, 7), (2 * n - 6, 2 * n + 10), (-5, -1), (70, 64), (0, 2 * n)]
            {
                let slice = s.slice(s.lower_bound(|x| x.cmp(&lo)), s.upper_bound(|x| x.cmp(&hi)));
                let expected: Vec<i64> = (0..n).map(|x| x * 2).filter(|x| *x >= lo && *x <= hi).collect();
                assert_eq!(slice.to_vec(), expected, "n {n} [{lo} {hi}]");
                assert_eq!(slice.count(), expected.len());
                assert_eq!(slice.is_empty(), expected.is_empty());
                assert_eq!(slice.first(), expected.first());
                let mut rev = expected.clone();
                rev.reverse();
                assert_eq!(slice.iter_rev().cloned().collect::<Vec<_>>(), rev, "rev n {n} [{lo} {hi}]");
            }
            let mut all: Vec<i64> = s.all().iter_rev().cloned().collect();
            all.reverse();
            assert_eq!(all, (0..n).map(|x| x * 2).collect::<Vec<_>>());
        }
    }

    #[test]
    fn inserts_after_from_sorted() {
        let mut s = SortedSet::from_sorted((0..1000i64).map(|x| x * 2).collect());
        for x in shuffled(1000) {
            assert!(s.insert(x * 2 + 1, &cmp));
        }
        assert_eq!(s.iter().cloned().collect::<Vec<_>>(), (0..2000).collect::<Vec<_>>());
    }
}
