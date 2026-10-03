//! What the module keeps between calls: the databases and functions the host holds handles to.
//!
//! A call that does not return (the crate's own words on it) may stop anywhere in here that calls anything, and
//! every table here is then whole all the same: a handle's entry is one value written, the handle of a value is a
//! number on the value itself (`Db::mark`) and no map beside the table, and the one map there is only ever grows.

use datascript::lock::Slot;
use datascript::value::{Func, WeakFunc};
use datascript::{Db, Error, Result};
use std::collections::HashMap;
use std::sync::atomic::Ordering::Relaxed;

/// Entries by handle. A handle that is given up is given out again.
pub struct Slab<T> {
    items: Vec<Option<T>>,
    free: Vec<u32>,
}

impl<T> Default for Slab<T> {
    fn default() -> Self {
        Slab { items: Vec::new(), free: Vec::new() }
    }
}

impl<T> Slab<T> {
    pub fn insert(&mut self, item: T) -> u32 {
        match self.free.pop() {
            Some(handle) => {
                self.items[handle as usize] = Some(item);
                handle
            }
            None => {
                self.items.push(Some(item));
                (self.items.len() - 1) as u32
            }
        }
    }

    pub fn get(&self, handle: u32) -> Option<&T> {
        self.items.get(handle as usize)?.as_ref()
    }

    pub fn get_mut(&mut self, handle: u32) -> Option<&mut T> {
        self.items.get_mut(handle as usize)?.as_mut()
    }

    pub fn remove(&mut self, handle: u32) -> Option<T> {
        // room for the handle first: once the entry is taken, the handle is given up with nothing more asked for
        self.free.reserve(1);
        let item = self.items.get_mut(handle as usize)?.take()?;
        self.free.push(handle);
        Some(item)
    }

    pub fn len(&self) -> usize {
        self.items.len() - self.free.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Default)]
pub struct State {
    /// A database value has one handle while the host holds it, and its mark is that handle and one: the same
    /// value is the same handle
    dbs: Slab<Db>,
    module_fns: Slab<Func>,
    /// The host's functions that are held here, by the host's handles: the same function is the same value. An
    /// entry is put in or written over, never taken out.
    pub host_fns: HashMap<u32, WeakFunc>,
}

impl State {
    /// The handle the host knows a database by. A value the host already holds keeps the handle it has; the host
    /// gives a handle back once, when it lets go of the value.
    pub fn db_handle(&mut self, db: &Db) -> u32 {
        let mark = db.mark().load(Relaxed);
        if mark != 0 {
            return mark - 1;
        }
        let handle = self.dbs.insert(db.clone());
        db.mark().store(handle + 1, Relaxed);
        handle
    }

    pub fn db(&self, handle: u32) -> Result<Db> {
        self.dbs.get(handle).cloned().ok_or_else(|| Error::msg(format!("datascript: no database of handle {handle}")))
    }

    /// The host gives a handle back. The database is the caller's to drop, outside the state's borrow: it may hold
    /// functions of the host's.
    pub fn release_db(&mut self, handle: u32) -> Option<Db> {
        let db = self.dbs.remove(handle)?;
        db.mark().store(0, Relaxed);
        Some(db)
    }

    pub fn module_fn_handle(&mut self, f: &Func) -> u32 {
        let mark = f.mark().load(Relaxed);
        if mark != 0 {
            return mark - 1;
        }
        let handle = self.module_fns.insert(f.clone());
        f.mark().store(handle + 1, Relaxed);
        handle
    }

    pub fn module_fn(&self, handle: u32) -> Result<Func> {
        self.module_fns
            .get(handle)
            .cloned()
            .ok_or_else(|| Error::msg(format!("datascript: no function of handle {handle}")))
    }

    pub fn release_module_fn(&mut self, handle: u32) -> Option<Func> {
        let f = self.module_fns.remove(handle)?;
        f.mark().store(0, Relaxed);
        Some(f)
    }

    /// How much is held: databases, functions. For a host that checks it gives back what it takes.
    pub fn held(&self) -> (usize, usize) {
        (self.dbs.len(), self.module_fns.len())
    }
}

thread_local! {
    static STATE: Slot<State> = Slot::new(State::default());
}

/// The state, for as long as `f` runs. `f` must not call the host: the host may call back, and the state is one.
pub fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    STATE.with(|s| f(&mut s.get()))
}
