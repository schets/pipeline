use std::cell::Cell;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

use util::maybe_acquire::{MAYBE_ACQUIRE, maybe_acquire_fence};
use util::consume::Consume;
use util::countedu16::{CountedU16, Transaction};

#[derive(Clone, Copy)]
enum ReaderState {
    Single,
    Multi,
}

/// This represents a single position in the queue
/// This packs two 16 bit elements into the AtomicUsize
/// - the #read and the current position. The #read makes
/// a sort of ABA counter for the current position
pub struct Reader {
    pos_data: CountedU16,
    state: Cell<ReaderState>,
    num_consumers: AtomicUsize,
}

/// This represents the reader attempt at loading a transaction
struct ReadAttempt<'a> {
    linked: Transaction<'a>,
    state: ReaderState,
}

/// This holds the set of readers currently active
struct ReaderGroup {
    // These pointers don't need ordering since they
    // are constant for a given ReaderGroup ptr
    readers: * const * const Reader,
    n_readers: usize,
}

#[repr(C)]
pub struct Cursor {
    readers: AtomicPtr<ReaderGroup>,
}

impl<'a> ReadAttempt<'a> {

    #[inline(always)]
    pub fn get(&self) -> u16 {
        self.linked.get()
    }

    #[inline(always)]
    pub fn get_wraps(&self) -> usize {
        self.linked.get_wraps()
    }


    #[inline(always)]
    pub fn commit_attempt(&self, by: u16, ord: Ordering) -> Option<ReadAttempt<'a>> {
        match self.state {
            Single => {
                self.linked.commit_direct(by, ord);
                None
            },
            Multi => {
                self.linked.commit(by, ord)
            }
        }
    }
}

impl Reader {
    #[inline(always)]
    pub fn load_attempt(&self, ord: Ordering) -> ReadAttempt {
        ReadAttempt {
            linked: self.pos_data.load_transaction(ord),
            state: self.state.get(),
        }
    }

    #[inline(always)]
    pub fn load_nread(&self, ord: Ordering) -> usize {
        self.pos_data.load_count(ord)
    }
}

impl ReaderGroup {
    pub fn get_max_diff(&self, _cur_writer: u16) -> Option<u16> {
        let cur_writer = _cur_writer as usize;
        let mut max_diff: usize = 0;
        unsafe {
            for i in 0..self.n_readers {
                // The difference is that this loads the total position
                // If a reader has passed the writer during this function call
                // then what must have happened is that somebody else has completed this
                // and we should instead retry (and reload the global ctr)
                let rpos = (**self.ptrs.offset(i)).pos_data.load_count(MAYBE_ACQUIRE);
                let diff = cur_writer.wrapping_sub(rpos);
                if diff > (::std::u16::MAX as usize) {
                    return None
                }
                max_diff = if diff > max_diff { diff } else { max_diff }
            }
        }
        maybe_acquire_fence();
        Some(max_diff as u16)
    }
}

impl Cursor {
    #[inline(always)]
    pub fn prefetch_metadata(&self) {
        unsafe {
            let rg = &*self.readers.load(Consume);
            ptr::read_volatile(&rg.n_readers);
        }
    }

    #[inline(always)]
    pub fn get_max_diff(&self, cur_writer: u16) -> Option<u16> {
        unsafe {
            let rg = &*self.readers.load(Consume);
            rg.get_max_diff(cur_writer)
        }
    }
}
