//! Locks and caches that a call which is cut short cannot leave shut.
//!
//! In a WebAssembly module the port runs on its host's stack. When that stack runs out while the host is inside the
//! module, a deep recursion of the host's own that asks the database something at every level, say, the host's
//! engine ends the call where it stands, and nothing is unwound. A `Mutex` that was locked would stay locked, a
//! `OnceLock` that was being set would stay unset and unsettable, a `RefCell` that was borrowed would stay borrowed:
//! every call after would fail. Nothing there needs a lock to keep another thread out, since there is none. So a
//! `Lock` is a mutex where there are threads and the value alone where there are not, a `Slot` likewise a `RefCell`
//! or the value alone, and `HashCache` and `Lazy` are set in one store, with no state in between.
//!
//! Where there are no threads, what these give out is exclusive only because the port never asks for one of them
//! again while it holds it. Where there are threads the mutex and the cell hold it to that, and the port's tests
//! run there.

use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
mod imp {
    use std::cell::{RefCell, RefMut};
    use std::sync::{Mutex, MutexGuard};

    pub struct Lock<T>(Mutex<T>);
    pub type Guard<'a, T> = MutexGuard<'a, T>;

    impl<T> Lock<T> {
        pub const fn new(value: T) -> Lock<T> {
            Lock(Mutex::new(value))
        }

        pub fn lock(&self) -> Guard<'_, T> {
            self.0.lock().unwrap_or_else(|e| e.into_inner())
        }

        pub fn get_mut(&mut self) -> &mut T {
            self.0.get_mut().unwrap_or_else(|e| e.into_inner())
        }
    }

    pub struct Slot<T>(RefCell<T>);
    pub type SlotGuard<'a, T> = RefMut<'a, T>;

    impl<T> Slot<T> {
        pub const fn new(value: T) -> Slot<T> {
            Slot(RefCell::new(value))
        }

        pub fn get(&self) -> SlotGuard<'_, T> {
            self.0.borrow_mut()
        }
    }
}

#[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
mod imp {
    use std::cell::UnsafeCell;

    pub struct Lock<T>(UnsafeCell<T>);

    // one thread: nothing is shared with another
    unsafe impl<T> Sync for Lock<T> {}
    unsafe impl<T> Send for Lock<T> {}

    pub struct Guard<'a, T>(&'a mut T);

    impl<T> std::ops::Deref for Guard<'_, T> {
        type Target = T;

        #[inline]
        fn deref(&self) -> &T {
            self.0
        }
    }

    impl<T> std::ops::DerefMut for Guard<'_, T> {
        #[inline]
        fn deref_mut(&mut self) -> &mut T {
            self.0
        }
    }

    impl<T> Lock<T> {
        pub const fn new(value: T) -> Lock<T> {
            Lock(UnsafeCell::new(value))
        }

        #[inline]
        pub fn lock(&self) -> Guard<'_, T> {
            // SAFETY: there is one thread, and the port does not lock what it holds locked
            Guard(unsafe { &mut *self.0.get() })
        }

        pub fn get_mut(&mut self) -> &mut T {
            self.0.get_mut()
        }
    }

    pub struct Slot<T>(UnsafeCell<T>);
    pub type SlotGuard<'a, T> = Guard<'a, T>;

    impl<T> Slot<T> {
        pub const fn new(value: T) -> Slot<T> {
            Slot(UnsafeCell::new(value))
        }

        #[inline]
        pub fn get(&self) -> SlotGuard<'_, T> {
            // SAFETY: a slot is a thread's own, and the port does not take what it holds
            Guard(unsafe { &mut *self.0.get() })
        }
    }
}

/// A value one holder at a time works on: a mutex where there are threads.
pub use imp::{Guard, Lock};
/// A value of a thread's own that one holder at a time works on: what a `thread_local!` keeps.
pub use imp::{Slot, SlotGuard};

/// ClojureScript's hash of a value, kept once it is computed.
pub struct HashCache(AtomicU64);

/// A hash that is kept: the bit above its 32 says so.
const KEPT: u64 = 1 << 32;

impl HashCache {
    pub const fn new() -> HashCache {
        HashCache(AtomicU64::new(0))
    }

    #[inline]
    pub fn get_or(&self, compute: impl FnOnce() -> i32) -> i32 {
        let kept = self.0.load(Ordering::Relaxed);
        if kept != 0 {
            return kept as u32 as i32;
        }
        let hash = compute();
        self.0.store(KEPT | hash as u32 as u64, Ordering::Relaxed);
        hash
    }
}

impl Default for HashCache {
    fn default() -> HashCache {
        HashCache::new()
    }
}

impl Clone for HashCache {
    /// A copy of a value hashes as the value does.
    fn clone(&self) -> HashCache {
        HashCache(AtomicU64::new(self.0.load(Ordering::Relaxed)))
    }
}

/// A value made the first time it is asked for and kept for good: a table the port builds once.
pub struct Lazy<T: 'static>(AtomicPtr<T>);

impl<T: Sync + 'static> Lazy<T> {
    pub const fn new() -> Lazy<T> {
        Lazy(AtomicPtr::new(std::ptr::null_mut()))
    }

    pub fn get_or_init(&self, make: impl FnOnce() -> T) -> &'static T {
        let kept = self.0.load(Ordering::Acquire);
        if !kept.is_null() {
            // SAFETY: what is kept is never freed
            return unsafe { &*kept };
        }
        let made = Box::into_raw(Box::new(make()));
        match self.0.compare_exchange(std::ptr::null_mut(), made, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => unsafe { &*made },
            // another thread made it first: its is the one kept
            Err(theirs) => {
                // SAFETY: `made` is this call's own, and nothing else has seen it
                drop(unsafe { Box::from_raw(made) });
                unsafe { &*theirs }
            }
        }
    }
}

impl<T: Sync + 'static> Default for Lazy<T> {
    fn default() -> Lazy<T> {
        Lazy::new()
    }
}
