//! What the module keeps between calls: the databases and functions the host holds handles to.

use datascript::value::{Func, WeakFunc};
use datascript::{Db, Error, Result};
use std::cell::RefCell;
use std::collections::HashMap;

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
        let item = self.items.get_mut(handle as usize)?.take()?;
        self.free.push(handle);
        Some(item)
    }

    pub fn len(&self) -> usize {
        self.items.len() - self.free.len()
    }
}

#[derive(Default)]
pub struct State {
    dbs: Slab<Db>,
    /// A database value has one handle while the host holds it: the same value is the same handle
    db_by_identity: HashMap<usize, u32>,
    module_fns: Slab<Func>,
    module_fn_by_id: HashMap<u32, u32>,
    /// The host's functions that are held here, by the host's handles: the same function is the same value
    pub host_fns: HashMap<u32, WeakFunc>,
}

impl State {
    /// The handle the host knows a database by. A value the host already holds keeps the handle it has; the host
    /// gives a handle back once, when it lets go of the value.
    pub fn db_handle(&mut self, db: &Db) -> u32 {
        let identity = db.identity();
        if let Some(handle) = self.db_by_identity.get(&identity).copied() {
            return handle;
        }
        let handle = self.dbs.insert(db.clone());
        self.db_by_identity.insert(identity, handle);
        handle
    }

    pub fn db(&self, handle: u32) -> Result<Db> {
        self.dbs.get(handle).cloned().ok_or_else(|| Error::msg(format!("datascript: no database of handle {handle}")))
    }

    /// The host gives a handle back. The database is the caller's to drop, outside the state's borrow: it may hold
    /// functions of the host's.
    pub fn release_db(&mut self, handle: u32) -> Option<Db> {
        let db = self.dbs.remove(handle)?;
        self.db_by_identity.remove(&db.identity());
        Some(db)
    }

    pub fn module_fn_handle(&mut self, f: &Func) -> u32 {
        if let Some(handle) = self.module_fn_by_id.get(&f.id()).copied() {
            return handle;
        }
        let handle = self.module_fns.insert(f.clone());
        self.module_fn_by_id.insert(f.id(), handle);
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
        self.module_fn_by_id.remove(&f.id());
        Some(f)
    }

    /// How much is held: databases, functions. For a host that checks it gives back what it takes.
    pub fn held(&self) -> (usize, usize) {
        (self.dbs.len(), self.module_fns.len())
    }
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// The state, for as long as `f` runs. `f` must not call the host: the host may call back, and the state is one.
pub fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    STATE.with(|s| f(&mut s.borrow_mut()))
}
