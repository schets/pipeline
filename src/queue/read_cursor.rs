use std::cell::Cell;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering, fence};

use util::alloc;
use util::consume::Consume;
use util::countedu16::{CountedU16, Transaction};
use util::maybe_acquire::{MAYBE_ACQUIRE, maybe_acquire_fence};

#[derive(Clone, Copy)]
enum ReaderState {
    Single,
    Multi,
}

pub struct Reader {
    pos_data: CountedU16,
    state: Cell<ReaderState>,
    num_consumers: AtomicUsize,
}

/// This represents the reader attempt at loading a transaction
/// It behaves similarly to a Transaction but has logic for single/multi
/// readers
struct ReadAttempt<'a> {
    linked: Transaction<'a>,
    reader: &'a Reader,
    state: ReaderState,
}

/// This holds the set of readers currently active.
/// This struct is held out of line from the cursor so it's easy to atomically replace it
struct ReaderGroup {
    readers: *const *const Reader,
    n_readers: usize,
}

#[repr(C)]
pub struct ReadCursor {
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
    pub fn commit_attempt(self, by: u16, ord: Ordering) -> Option<ReadAttempt<'a>> {
        match self.state {
            ReaderState::Single => {
                self.linked.commit_direct(by, ord);
                None
            }
            ReaderState::Multi => {
                if self.reader.num_consumers.load(Ordering::Relaxed) == 1 {
                    fence(Ordering::Acquire);
                    self.reader.state.set(ReaderState::Single);
                    self.linked.commit_direct(by, ord);
                    None
                } else {
                    match self.linked.commit(by, ord) {
                        Some(transaction) => {
                            Some(ReadAttempt {
                                linked: transaction,
                                reader: self.reader,
                                state: ReaderState::Multi,
                            })
                        }
                        None => None,
                    }
                }
            }
        }
    }
}

impl Reader {
    #[inline(always)]
    pub fn load_attempt(&self, ord: Ordering) -> ReadAttempt {
        ReadAttempt {
            linked: self.pos_data.load_transaction(ord),
            reader: self,
            state: self.state.get(),
        }
    }

    #[inline(always)]
    pub fn load_nread(&self, ord: Ordering) -> usize {
        self.pos_data.load_count(ord)
    }
}

impl ReaderGroup {
    pub fn new() -> ReaderGroup {
        ReaderGroup {
            readers: ptr::null(),
            n_readers: 0,
        }
    }

    /// Only safe to call from a consumer of the queue!
    pub unsafe fn add_reader(&self,
                             raw: usize,
                             wrap: u16)
                             -> (*mut ReaderGroup, AtomicPtr<Reader>) {
        let next_readers = self.n_readers + 1;
        let new_reader = alloc::allocate(1);
        let new_readers = alloc::allocate(next_readers);
        let new_group = alloc::allocate(1);
        ptr::write(new_reader,
                   Reader {
                       pos_data: CountedU16::from_usize(raw, wrap),
                       state: Cell::new(ReaderState::Single),
                       num_consumers: AtomicUsize::new(1),
                   });
        for i in 0..self.n_readers as isize {
            *new_readers.offset(i) = *self.readers.offset(i);
        }
        *new_readers.offset((next_readers - 1) as isize) = new_reader;
        ptr::write(new_group,
                   ReaderGroup {
                       readers: new_readers as *const *const Reader,
                       n_readers: next_readers,
                   });
        (new_group, AtomicPtr::new(new_reader))
    }

    pub fn get_max_diff(&self, _cur_writer: u16) -> Option<u16> {
        let cur_writer = _cur_writer as usize;
        let mut max_diff: usize = 0;
        unsafe {
            for i in 0..self.n_readers as isize {
                // If a reader has passed the writer during this function call
                // then what must have happened is that somebody else has completed this
                // written to the queue, and a reader has bypassed it. We should retry
                let rpos = (**self.readers.offset(i)).pos_data.load_count(MAYBE_ACQUIRE);
                let diff = cur_writer.wrapping_sub(rpos);
                if diff > (::std::u16::MAX as usize) {
                    return None;
                }
                max_diff = if diff > max_diff { diff } else { max_diff }
            }
        }
        maybe_acquire_fence();
        Some(max_diff as u16)
    }
}

impl ReadCursor {
    pub fn new(wrap: u16) -> (ReadCursor, AtomicPtr<Reader>) {
        let rg = ReaderGroup::new();
        unsafe {
            let (real_group, reader) = rg.add_reader(0, wrap);
            (ReadCursor { readers: AtomicPtr::new(real_group) }, reader)
        }
    }

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

    pub fn add_reader(&self, reader: &Reader) -> AtomicPtr<Reader> {
        // This leaks extremely badly here.
        // Freeing during runtime will probably use the memory management system
        // Freeing before could just free if the server hasn't been started
        let mut current_ptr = self.readers.load(Consume);
        loop {
            unsafe {
                let current_group = &*current_ptr;
                let raw = reader.pos_data.load_raw(Ordering::Relaxed);
                let wrap = reader.pos_data.wrap_at();
                let (new_group, new_reader) = current_group.add_reader(raw, wrap);
                match self.readers
                    .compare_exchange(current_ptr, new_group, Ordering::Release, Consume) {
                    Ok(_) => return new_reader,
                    Err(val) => current_ptr = val,
                }
            }
        }
    }
}
