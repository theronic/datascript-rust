//! The module's memory allocator.
//!
//! The module runs on its host's stack, and a host's stack can run out under it (the crate's own words on a call
//! that does not return). The engine then ends the call at whichever function was about to be entered, with
//! nothing unwound. An allocator that calls its own functions while its lists are half changed — the standard
//! library's does, to take a block out of one list and put what is left of it into another — can be stopped right
//! there, and what it then hands out is no longer its to hand out.
//!
//! This one calls nothing while anything of its own is half changed. Taking memory (`Heap::alloc`) and giving it
//! back (`Heap::free`) are each one function with no call in it, so each is entered or it is not, and one that is
//! entered finishes; changing a size is those two, each whole, where it is not done in place. That is the whole of
//! the reason for it, and the test of it is in `script/test_rust.sh`: the allocator's functions in the built module
//! are read (`js/leaf-check.mjs`), and they call nothing but those two.
//!
//! It keeps memory the way TLSF does (M. Masmano, I. Ripoll, A. Crespo and J. Real, "TLSF: a new dynamic memory
//! allocator for real-time systems", 2004), which finds a block without searching for one and so needs no function
//! to search in:
//!
//! - Memory is blocks, one after another. A block begins with a word that says how long it is, whether it is free,
//!   and whether the block before it is. A free block also holds the two free blocks beside it in its list, and at
//!   its very end where it starts, which is how the block after it finds it.
//! - A block that is given back is joined to the free blocks on either side of it, so no two free blocks are ever
//!   side by side, and what a long vector leaves behind as it grows is whole again for the next.
//! - The free blocks are in lists by length: a list for every length below 256 bytes, and above that 32 lists to
//!   each doubling. Two words of bits say which lists have anything, so the first list whose blocks are all long
//!   enough is found by counting zeros. The block taken is cut to the length asked, and the rest is a block again.
//! - The free block at the end of memory is on no list, and is cut from only when no other block will do, as the
//!   standard library's allocator keeps its own: it is what more pages lengthen, and left whole it is where the
//!   next long thing goes. Without that a database that is changed and written out by turns, the short blocks of
//!   the one scattered through what the other gave back, took a third more memory than it does (`docs/rust.md`).
//! - Memory comes from the host a page at a time and is never given back to it, as WebAssembly's memory never is.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;

/// A WebAssembly page.
const PAGE: usize = 65536;
const WORD: usize = core::mem::size_of::<usize>();
/// What every block's length is a multiple of, and what every answer is aligned to: 8 on WebAssembly
const ALIGN: usize = 2 * WORD;
const ALIGN_LOG2: u32 = ALIGN.trailing_zeros();
/// The shortest block: its length, its two neighbours in its list, and where it starts
const MIN: usize = 4 * WORD;
const SECOND_LOG2: u32 = 5;
/// How many lists a doubling of length has
const SECOND: usize = 1 << SECOND_LOG2;
const SMALL_LOG2: u32 = SECOND_LOG2 + ALIGN_LOG2;
/// Below this every length has a list of its own
const SMALL: usize = 1 << SMALL_LOG2;
/// How many doublings there are lists for: every length an address can be
const FIRST: usize = (usize::BITS - SMALL_LOG2 + 1) as usize;
/// The most that is answered at once: a quarter of what can be addressed
const MAX: usize = 1 << (usize::BITS - 2);
/// How many blocks of the list of its own length a request looks at before it takes a longer block
const LOOKED_AT: usize = 8;

/// In a block's first word, beside its length: it is free
const FREE: usize = 1;
/// and the block before it is
const BEFORE_FREE: usize = 2;
const FLAGS: usize = FREE | BEFORE_FREE;

/// Where memory comes from.
pub trait Pages {
    /// `bytes` more, which is whole pages: where they start, or `None` when there are no more to be had.
    fn more(&mut self, bytes: usize) -> Option<usize>;
}

pub struct Heap<P> {
    pages: P,
    /// Which doublings have a list with a block in it
    first: usize,
    /// and which of each doubling's lists have
    second: [u32; FIRST],
    /// The first free block of each list
    lists: [[usize; SECOND]; FIRST],
    /// Where the pages last taken end: more pages that start here lengthen the last block
    top: usize,
    /// The free block at the end of memory, when there is one. It is on no list: it is what memory grows by, and
    /// it is cut from only when no other block will do, so that what is long stays in one piece
    last: usize,
    taken: usize,
}

#[inline(always)]
unsafe fn word(at: usize) -> usize {
    *(at as *const usize)
}

#[inline(always)]
unsafe fn set_word(at: usize, to: usize) {
    *(at as *mut usize) = to
}

/// The block that holds `n` bytes: its first word and them, in whole multiples of the alignment.
#[inline(always)]
const fn block_for(n: usize) -> usize {
    let len = (n + WORD + ALIGN - 1) & !(ALIGN - 1);
    if len < MIN {
        MIN
    } else {
        len
    }
}

/// The list a free block of a length is in.
#[inline(always)]
const fn list_of(len: usize) -> (usize, usize) {
    if len < SMALL {
        (0, len >> ALIGN_LOG2)
    } else {
        let top = usize::BITS - 1 - len.leading_zeros();
        ((top - (SMALL_LOG2 - 1)) as usize, (len >> (top - SECOND_LOG2)) ^ SECOND)
    }
}

/// The length whose list, and every list after it, holds only blocks at least `len` long.
#[inline(always)]
const fn rounded_up(len: usize) -> usize {
    if len < SMALL {
        len
    } else {
        len + (1 << (usize::BITS - 1 - len.leading_zeros() - SECOND_LOG2)) - 1
    }
}

impl<P: Pages> Heap<P> {
    pub const fn new(pages: P) -> Heap<P> {
        Heap { pages, first: 0, second: [0; FIRST], lists: [[0; SECOND]; FIRST], top: 0, last: 0, taken: 0 }
    }

    /// How much memory was taken from the host in all.
    pub fn taken(&self) -> usize {
        self.taken
    }

    /// The memory from `block`, `len` long, is a free block: written as one, and put on its list.
    #[inline(always)]
    unsafe fn put(&mut self, block: usize, len: usize) {
        set_word(block, len | FREE);
        set_word(block + len - WORD, block);
        set_word(block + len, word(block + len) | BEFORE_FREE);
        if block + len + WORD == self.top {
            self.last = block;
            return;
        }
        self.list(block, len);
    }

    /// A free block is put on the list of its length.
    #[inline(always)]
    unsafe fn list(&mut self, block: usize, len: usize) {
        let (first, second) = list_of(len);
        let list = self.lists.get_unchecked_mut(first).get_unchecked_mut(second);
        let next = *list;
        set_word(block + WORD, next);
        set_word(block + 2 * WORD, 0);
        if next != 0 {
            set_word(next + 2 * WORD, block);
        }
        *list = block;
        *self.second.get_unchecked_mut(first) |= 1 << second;
        self.first |= 1 << first;
    }

    /// A free block, `len` long, is taken off its list.
    #[inline(always)]
    unsafe fn take(&mut self, block: usize, len: usize) {
        if block == self.last {
            self.last = 0;
            return;
        }
        let (next, before) = (word(block + WORD), word(block + 2 * WORD));
        if next != 0 {
            set_word(next + 2 * WORD, before);
        }
        if before != 0 {
            set_word(before + WORD, next);
        } else {
            let (first, second) = list_of(len);
            *self.lists.get_unchecked_mut(first).get_unchecked_mut(second) = next;
            if next == 0 {
                let lists = self.second.get_unchecked_mut(first);
                *lists &= !(1 << second);
                if *lists == 0 {
                    self.first &= !(1 << first);
                }
            }
        }
    }

    /// A block that was in use becomes free: joined to the free blocks on either side of it.
    #[inline(always)]
    unsafe fn release(&mut self, block: usize) {
        let mut block = block;
        let head = word(block);
        let mut len = head & !FLAGS;
        if head & BEFORE_FREE != 0 {
            let before = word(block - WORD);
            let its = word(before) & !FLAGS;
            self.take(before, its);
            block = before;
            len += its;
        }
        let after = word(block + len);
        if after & FREE != 0 {
            let its = after & !FLAGS;
            self.take(block + len, its);
            len += its;
        }
        self.put(block, len);
    }

    /// More pages, so that the block at the end of memory is free and `len` long: no more of them than that takes,
    /// as the standard library's allocator asks. Pages that follow on from the last lengthen what is there, and
    /// what is free there counts towards the block; others are memory of their own, with a block of no length at
    /// their end that is never free.
    #[inline(always)]
    unsafe fn grow(&mut self, len: usize) -> bool {
        let mut there = if self.last != 0 { word(self.last) & !FLAGS } else { 0 };
        loop {
            let bytes = (len - there + ALIGN + PAGE - 1) & !(PAGE - 1);
            let Some(start) = self.pages.more(bytes) else { return false };
            self.taken += bytes;
            let follows = start == self.top;
            let block = if follows {
                // what marked the end is where the new block starts
                let block = start - WORD;
                set_word(block, bytes | (word(block) & BEFORE_FREE));
                block
            } else {
                // what was the end of memory is so no longer, and its last block is a block like any other
                if self.last != 0 {
                    self.list(self.last, word(self.last) & !FLAGS);
                    self.last = 0;
                }
                set_word(start + WORD, bytes - ALIGN);
                start + WORD
            };
            self.top = start + bytes;
            set_word(self.top - WORD, 0);
            self.release(block);
            if follows || there == 0 {
                return true;
            }
            // the pages did not follow on, so what was free at the end is no part of the block: once more, for all
            there = 0;
        }
    }

    /// Memory of a size and an alignment: null when there is none.
    ///
    /// # Safety
    /// `align` is a power of two, as a `Layout`'s is.
    #[inline(never)]
    pub unsafe fn alloc(&mut self, size: usize, align: usize) -> *mut u8 {
        if size > MAX || align > PAGE {
            return core::ptr::null_mut();
        }
        let need = block_for(size);
        // An alignment beyond the blocks' own is a place further into a block: what is before the place has to be
        // long enough to stay behind as a block
        let want = if align > ALIGN { need + align + MIN } else { need };
        let from = rounded_up(want);
        let mut asked = false;
        let (mut block, mut len) = 'found: {
            if need >= SMALL && align <= ALIGN {
                // The list of its own length holds shorter blocks too, so the search starts at the list after;
                // but its first few blocks are looked at, since a block just given back is often asked for again,
                // and one that fits there leaves a longer one whole
                let (first, second) = list_of(need);
                let mut block = *self.lists.get_unchecked(first).get_unchecked(second);
                let mut looked = 0;
                while block != 0 && looked < LOOKED_AT {
                    let len = word(block) & !FLAGS;
                    if len >= need {
                        self.take(block, len);
                        break 'found (block, len);
                    }
                    block = word(block + WORD);
                    looked += 1;
                }
            }
            loop {
                // the first list that has a block, from the one whose blocks are all long enough
                let (mut first, second) = list_of(from);
                let mut lists = *self.second.get_unchecked(first) & (!0u32 << second);
                if lists == 0 {
                    let above = self.first & (!0usize << (first + 1));
                    if above != 0 {
                        first = above.trailing_zeros() as usize;
                        lists = *self.second.get_unchecked(first);
                    }
                }
                if lists != 0 {
                    let block = *self.lists.get_unchecked(first).get_unchecked(lists.trailing_zeros() as usize);
                    let len = word(block) & !FLAGS;
                    self.take(block, len);
                    break 'found (block, len);
                }
                // no other block will do: the one at the end of memory, which more pages lengthen
                if self.last != 0 {
                    let (block, len) = (self.last, word(self.last) & !FLAGS);
                    if len >= want {
                        self.last = 0;
                        break 'found (block, len);
                    }
                }
                if asked || !self.grow(want) {
                    return core::ptr::null_mut();
                }
                asked = true;
            }
        };
        // The block is this call's: the one before it is in use, as a free block's neighbours are, and the one
        // after it says the one before it is free, which is put right below
        let mut before = 0;
        if align > ALIGN {
            let at = block + WORD;
            let mut place = (at + align - 1) & !(align - 1);
            if place != at && place - at < MIN {
                place += align;
            }
            let gap = place - at;
            if gap != 0 {
                self.put(block, gap);
                block += gap;
                len -= gap;
                before = BEFORE_FREE;
            }
        }
        if len - need >= MIN {
            // what is left over is a block again
            set_word(block, need | before);
            self.put(block + need, len - need);
        } else {
            set_word(block, len | before);
            set_word(block + len, word(block + len) & !BEFORE_FREE);
        }
        (block + WORD) as *mut u8
    }

    /// Gives back what `alloc` or `realloc` answered.
    ///
    /// # Safety
    /// `ptr` is what one of them answered, and was not given back since.
    #[inline(never)]
    pub unsafe fn free(&mut self, ptr: *mut u8) {
        self.release(ptr as usize - WORD);
    }

    /// The same bytes in memory of another size: where they are, when what follows them is free or they are
    /// to be fewer, and otherwise in memory asked for, with what they were in given back. Those two are asked for
    /// whole, with nothing half done between them.
    ///
    /// # Safety
    /// As `free`, of `ptr`; `old` and `align` are what it was asked with.
    #[inline(always)]
    pub unsafe fn realloc(&mut self, ptr: *mut u8, old: usize, align: usize, new: usize) -> *mut u8 {
        if new <= MAX {
            let block = ptr as usize - WORD;
            let head = word(block);
            let (len, before) = (head & !FLAGS, head & BEFORE_FREE);
            let need = block_for(new);
            if need <= len {
                // what it gives up is a block again, when it is long enough to be one
                if len - need >= MIN {
                    set_word(block, need | before);
                    set_word(block + need, len - need);
                    self.release(block + need);
                }
                return ptr;
            }
            let mut asked = false;
            loop {
                let after = word(block + len);
                let room = if after & FREE != 0 { after & !FLAGS } else { 0 };
                if len + room >= need {
                    // a vector that grows at the end of what is in use is not copied each time it doubles
                    self.take(block + len, room);
                    let both = len + room;
                    if both - need >= MIN {
                        set_word(block, need | before);
                        self.put(block + need, both - need);
                    } else {
                        set_word(block, both | before);
                        set_word(block + both, word(block + both) & !BEFORE_FREE);
                    }
                    return ptr;
                }
                // what follows it reaches the end of memory, or it does itself: pages that follow on lengthen it
                if asked || block + len + room + WORD != self.top || !self.grow(need - len) {
                    break;
                }
                asked = true;
            }
        }
        let moved = self.alloc(new, align);
        if !moved.is_null() {
            core::ptr::copy_nonoverlapping(ptr, moved, if old < new { old } else { new });
            self.free(ptr);
        }
        moved
    }
}

/// WebAssembly's memory, grown a page at a time.
#[cfg(target_arch = "wasm32")]
pub struct Memory;

#[cfg(target_arch = "wasm32")]
impl Pages for Memory {
    #[inline(always)]
    fn more(&mut self, bytes: usize) -> Option<usize> {
        let pages = bytes / PAGE;
        // the last page there can be is left alone: where it ends is not a number an address can be
        if core::arch::wasm32::memory_size(0) + pages >= 65536 {
            return None;
        }
        let before = core::arch::wasm32::memory_grow(0, pages);
        if before == usize::MAX {
            None
        } else {
            Some(before * PAGE)
        }
    }
}

/// The heap as the module's allocator: the module is one thread, and nothing here is entered twice.
pub struct ModuleHeap<P>(UnsafeCell<Heap<P>>);

unsafe impl<P> Sync for ModuleHeap<P> {}

impl<P: Pages> ModuleHeap<P> {
    pub const fn new(pages: P) -> ModuleHeap<P> {
        ModuleHeap(UnsafeCell::new(Heap::new(pages)))
    }
}

unsafe impl<P: Pages> GlobalAlloc for ModuleHeap<P> {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        (*self.0.get()).alloc(layout.size(), layout.align())
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, _: Layout) {
        (*self.0.get()).free(ptr)
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = (*self.0.get()).alloc(layout.size(), layout.align());
        if !ptr.is_null() {
            core::ptr::write_bytes(ptr, 0, layout.size());
        }
        ptr
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        (*self.0.get()).realloc(ptr, layout.size(), layout.align(), new_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Memory for a test: one long allocation, handed out a page at a time, with a hole left now and then when it
    /// is asked to, as a host that grows the memory itself would leave one.
    struct Arena {
        base: usize,
        used: usize,
        len: usize,
        holes: bool,
        asked: usize,
        /// Each stretch of pages that follow on from one another: where it starts and ends
        stretches: Vec<(usize, usize)>,
    }

    impl Arena {
        fn new(len: usize, holes: bool) -> Arena {
            let layout = std::alloc::Layout::from_size_align(len, PAGE).unwrap();
            let base = unsafe { std::alloc::alloc(layout) } as usize;
            assert!(base != 0);
            Arena { base, used: 0, len, holes, asked: 0, stretches: Vec::new() }
        }
    }

    impl Pages for Arena {
        fn more(&mut self, bytes: usize) -> Option<usize> {
            assert_eq!(bytes % PAGE, 0);
            self.asked += 1;
            if self.holes && self.asked.is_multiple_of(3) {
                self.used += PAGE;
            }
            if self.used + bytes > self.len {
                return None;
            }
            let start = self.base + self.used;
            self.used += bytes;
            match self.stretches.last_mut() {
                Some(last) if last.1 == start => last.1 += bytes,
                _ => self.stretches.push((start, start + bytes)),
            }
            Some(start)
        }
    }

    impl Heap<Arena> {
        /// Reads all of the heap, block by block and list by list, and says what is wrong with it if anything is.
        /// How much is in use, first words and all.
        unsafe fn check(&self) -> usize {
            let mut free = HashSet::new();
            let mut in_use = 0;
            for &(start, end) in &self.pages.stretches {
                let mut block = start + WORD;
                let mut before_free = false;
                while block != end - WORD {
                    let head = word(block);
                    let len = head & !FLAGS;
                    assert!(len >= MIN && len.is_multiple_of(ALIGN), "a block {len} long");
                    assert!(block + len <= end - WORD, "a block past the end of its memory");
                    assert_eq!(head & BEFORE_FREE != 0, before_free, "what a block says of the one before it");
                    if head & FREE != 0 {
                        assert!(!before_free, "two free blocks side by side");
                        assert_eq!(word(block + len - WORD), block, "where a free block says it starts");
                        free.insert(block);
                    } else {
                        in_use += len;
                    }
                    before_free = head & FREE != 0;
                    block += len;
                }
                assert_eq!(word(block) & !BEFORE_FREE, 0, "the end of memory");
                assert_eq!(word(block) & BEFORE_FREE != 0, before_free);
            }
            for first in 0..FIRST {
                for second in 0..SECOND {
                    let mut block = self.lists[first][second];
                    assert_eq!(block != 0, self.second[first] & (1 << second) != 0, "a list and its bit");
                    let mut before = 0;
                    while block != 0 {
                        assert!(free.remove(&block), "a block on a list that is not free, or is on two");
                        assert_eq!(list_of(word(block) & !FLAGS), (first, second), "a block on the wrong list");
                        assert_eq!(word(block + 2 * WORD), before);
                        before = block;
                        block = word(block + WORD);
                    }
                }
                assert_eq!(self.second[first] != 0, self.first & (1 << first) != 0, "a doubling and its bit");
            }
            if self.last != 0 {
                assert!(free.remove(&self.last), "the last block is free");
                assert_eq!(self.last + (word(self.last) & !FLAGS) + WORD, self.top);
            }
            assert!(free.is_empty(), "a free block on no list");
            in_use
        }
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    #[test]
    fn a_list_holds_the_lengths_it_says() {
        let mut before = (0, 0);
        for len in (MIN..(1 << 22)).step_by(ALIGN) {
            let list = list_of(len);
            assert!(list >= before && list.0 < FIRST && list.1 < SECOND, "{len}: {list:?} after {before:?}");
            before = list;
            // every block on the list a search starts from is long enough
            let from = list_of(rounded_up(len));
            assert!(from >= list);
            assert!(len == MIN || list_of(len - ALIGN) < from, "{len}: a shorter block would be taken for it");
        }
        assert!(list_of(usize::MAX & !(ALIGN - 1)).0 < FIRST);
    }

    struct Held {
        ptr: *mut u8,
        size: usize,
        align: usize,
        mark: u8,
    }

    unsafe fn fill(h: &Held) {
        for i in 0..h.size {
            *h.ptr.add(i) = h.mark.wrapping_add(i as u8);
        }
    }

    unsafe fn check(h: &Held, upto: usize) {
        for i in 0..upto.min(h.size) {
            assert_eq!(*h.ptr.add(i), h.mark.wrapping_add(i as u8), "a block of {} was written over at {i}", h.size);
        }
    }

    /// Blocks asked for, grown, shrunk and given back in no order, each filled with bytes of its own that have to
    /// be there when it is looked at again, and the whole heap read through now and then. What was taken, over the
    /// most that was in use at once.
    fn at_random(seed: u64, holes: bool, steps: usize) -> f64 {
        let mut heap = Heap::new(Arena::new(128 << 20, holes));
        let mut rng = Rng(seed);
        let mut held: Vec<Held> = Vec::new();
        let (mut live, mut most) = (0usize, 0usize);
        let size = |rng: &mut Rng| match rng.below(100) {
            0..=69 => 1 + rng.below(200),
            70..=93 => 1 + rng.below(6_000),
            94..=98 => 1 + rng.below(100_000),
            _ => 1 + rng.below(3 << 20),
        };
        let align = |rng: &mut Rng| match rng.below(20) {
            0..=11 => 8,
            12..=14 => 4,
            15 => 1,
            16..=17 => 16,
            18 => 32,
            _ => 256,
        };
        for step in 0..steps {
            let what = rng.below(100);
            unsafe {
                if held.is_empty() || what < 45 && live < (24 << 20) {
                    let (size, align) = (size(&mut rng), align(&mut rng));
                    let ptr = heap.alloc(size, align);
                    assert!(!ptr.is_null(), "step {step}: no memory for {size}");
                    assert_eq!(ptr as usize % align.max(ALIGN), 0, "step {step}: {size} aligned to {align}");
                    let h = Held { ptr, size, align, mark: rng.next() as u8 };
                    fill(&h);
                    live += size;
                    held.push(h);
                } else if what < 75 {
                    let h = held.swap_remove(rng.below(held.len()));
                    check(&h, usize::MAX);
                    heap.free(h.ptr);
                    live -= h.size;
                } else {
                    let i = rng.below(held.len());
                    let new = match rng.below(3) {
                        0 => held[i].size + 1 + rng.below(held[i].size + 16),
                        1 => 1 + rng.below(held[i].size),
                        _ => size(&mut rng),
                    };
                    let h = &mut held[i];
                    let ptr = heap.realloc(h.ptr, h.size, h.align, new);
                    assert!(!ptr.is_null(), "step {step}: no memory for {new}, {live} in use, {} taken", heap.taken());
                    assert_eq!(ptr as usize % h.align.max(ALIGN), 0);
                    let kept = h.size.min(new);
                    live = live - h.size + new;
                    h.ptr = ptr;
                    h.size = new;
                    check(h, kept);
                    fill(h);
                }
                if step % 997 == 0 {
                    let in_use = heap.check();
                    assert!(in_use >= live && in_use <= live + held.len() * (WORD + 2 * ALIGN + MIN));
                }
            }
            most = most.max(live);
        }
        unsafe {
            for h in &held {
                check(h, usize::MAX);
            }
            heap.check();
            // and with everything given back, each stretch of memory is one free block
            for h in held.drain(..) {
                heap.free(h.ptr);
            }
            assert_eq!(heap.check(), 0);
            for &(start, end) in &heap.pages.stretches {
                assert_eq!(word(start + WORD), (end - start - ALIGN) | FREE);
            }
        }
        heap.taken() as f64 / most as f64
    }

    /// Fewer steps where every byte written is checked for overflow besides
    const STEPS: usize = if cfg!(debug_assertions) { 40_000 } else { 300_000 };

    #[test]
    fn blocks_keep_what_is_written_to_them() {
        for seed in [0x9E37_79B9_7F4A_7C15, 0x2545_F491_4F6C_DD1D] {
            let over = at_random(seed, false, STEPS);
            // What was taken is what was in use at the most, and what was free between the blocks. It is a good deal
            // here, where blocks of every length up to many megabytes come and go in no order: nothing a database
            // does, and the bound is against a heap that gives nothing back, not for one that packs this well.
            assert!(over < 2.0, "{over} times what was in use at the most");
        }
    }

    #[test]
    fn memory_that_comes_with_holes_in_it() {
        let over = at_random(0x1234_5678_9ABC_DEF1, true, STEPS);
        assert!(over < 2.4, "{over} times what was in use at the most");
    }

    #[test]
    fn what_is_given_back_is_given_out_again() {
        let mut heap = Heap::new(Arena::new(16 << 20, false));
        let sizes: Vec<usize> = (0..4000).map(|i| 1 + (i * 37) % 3000).collect();
        unsafe {
            let first: Vec<*mut u8> = sizes.iter().map(|s| heap.alloc(*s, 8)).collect();
            let taken = heap.taken();
            for ptr in &first {
                heap.free(*ptr);
            }
            assert_eq!(heap.check(), 0);
            for _ in 0..50 {
                let again: Vec<*mut u8> = sizes.iter().rev().map(|s| heap.alloc(*s, 8)).collect();
                for ptr in &again {
                    heap.free(*ptr);
                }
            }
            assert_eq!(heap.taken(), taken);
            assert_eq!(heap.check(), 0);
        }
    }

    #[test]
    fn blocks_given_back_are_joined() {
        let mut heap = Heap::new(Arena::new(64 << 20, false));
        unsafe {
            // nine blocks side by side, given back in an order that leaves holes before it fills them
            let blocks: Vec<*mut u8> = (0..9).map(|_| heap.alloc(1 << 20, 8)).collect();
            let after = heap.alloc(40_000, 8);
            let taken = heap.taken();
            for i in [1, 3, 5, 7, 0, 8, 4, 2, 6] {
                heap.free(blocks[i]);
                heap.check();
            }
            // one block of all nine
            let whole = heap.alloc(9 << 20, 8);
            assert_eq!(whole, blocks[0]);
            assert_eq!(heap.taken(), taken);
            heap.free(whole);
            // and short blocks are cut from it
            let short: Vec<*mut u8> = (0..60_000).map(|_| heap.alloc(100, 8)).collect();
            assert!(short.iter().all(|p| !p.is_null()));
            assert_eq!(heap.taken(), taken, "6 MB of short blocks out of the 9 MB given back");
            heap.free(after);
            heap.check();
        }
    }

    #[test]
    fn a_block_grows_where_it_is() {
        let mut heap = Heap::new(Arena::new(64 << 20, false));
        unsafe {
            let _first = heap.alloc(40, 8);
            // a vector doubling, with nothing after it but what is free and the end of memory
            let mut ptr = heap.alloc(1000, 8);
            let at = ptr;
            let mut size = 1000;
            while size < (20 << 20) {
                *ptr.add(size - 1) = 7;
                ptr = heap.realloc(ptr, size, 8, size * 2);
                assert_eq!(ptr, at, "growing from {size}");
                assert_eq!(*ptr.add(size - 1), 7);
                size *= 2;
            }
            // and what it took is what it is long, not each length it has been
            assert!(heap.taken() < size + size / 8, "{} taken for {size}", heap.taken());
            let shorter = heap.realloc(ptr, size, 8, 50_000);
            assert_eq!(shorter, at);
            let next = heap.alloc(100_000, 8);
            assert_eq!(next as usize, at as usize + block_for(50_000), "what it gave up is the next block");
            heap.check();
        }
    }

    #[test]
    fn an_alignment_beyond_the_blocks_own() {
        let mut heap = Heap::new(Arena::new(8 << 20, false));
        unsafe {
            let mut held = Vec::new();
            for (i, align) in [16usize, 32, 64, 256, 4096, 65536, 16, 4096].into_iter().enumerate() {
                let _between = heap.alloc(24 + i, 8);
                let size = 100 + i * 1000;
                let ptr = heap.alloc(size, align);
                assert!(!ptr.is_null());
                assert_eq!(ptr as usize % align, 0, "{size} aligned to {align}");
                core::ptr::write_bytes(ptr, i as u8, size);
                held.push((ptr, size, align));
                heap.check();
            }
            for (i, (ptr, size, align)) in held.iter_mut().enumerate() {
                let grown = heap.realloc(*ptr, *size, *align, *size * 3);
                assert_eq!(grown as usize % *align, 0);
                assert_eq!(*grown.add(*size - 1), i as u8);
                *ptr = grown;
                heap.check();
            }
            for (ptr, _, _) in held {
                heap.free(ptr);
            }
            heap.check();
        }
    }

    #[test]
    fn no_memory_is_an_answer() {
        let mut heap = Heap::new(Arena::new(1 << 20, false));
        unsafe {
            assert!(heap.alloc(2 << 20, 8).is_null());
            assert!(heap.alloc(MAX + 1, 8).is_null());
            assert!(heap.alloc(100, 2 * PAGE).is_null());
            let mut held = Vec::new();
            loop {
                let ptr = heap.alloc(5000, 8);
                if ptr.is_null() {
                    break;
                }
                held.push(ptr);
            }
            assert!(held.len() > 190, "{} blocks of 5000 in 1 MB", held.len());
            heap.check();
            for ptr in held.drain(..) {
                heap.free(ptr);
            }
            assert_eq!(heap.check(), 0);
            assert!(!heap.alloc(900_000, 8).is_null());
        }
    }
}
