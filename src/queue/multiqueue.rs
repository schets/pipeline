
use std::cell::Cell;
use std::mem;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, fence};
use std::sync::atomic::Ordering::{Relaxed, Acquire, Release};

use util::countedu16::CountedU16;
use util::maybe_acquire::{maybe_acquire_fence, MAYBE_ACQUIRE};

use queue::read_cursor::{ReadCursor, Reader};

#[derive(Clone, Copy)]
enum QueueState {
    Single,
    Multi,
}

struct QueueEntry<T> {
    val: T,
    wraps: AtomicUsize,
}

/// A bounded queue that supports multiple reader and writers
/// and supports effecient methods for single consumers and producers
#[repr(C)]
struct MultiQueue<T> {
    d1: [u8; 64],

    // Writer data
    head: CountedU16,
    tail_cache: AtomicUsize,
    writers: AtomicUsize,
    d2: [u8; 64],

    // Shared Data
    // The data and the wraps flag are in the same location
    // to reduce the # of distinct cache lines read when getting an item
    // The tail itself is rarely modified, making it a suitable candidate
    // to be in the shared space
    tail: ReadCursor,
    data: *mut QueueEntry<T>,
    capacity: isize,
    backlog_check: isize,
    d3: [u8; 64],
}

pub struct MultiWriter<T> {
    queue: Arc<MultiQueue<T>>,
    state: Cell<QueueState>,
}

/*
pub struct MultiReader<'a, T> {
    queue: Arc<MultiQueue<T>>,
    reader: &'a Reader,
    state: Cell<QueueState>,
}*/

impl<T> MultiQueue<T> {
    /*
    pub fn new(capacity: u16) -> (MultiWriter<T>, MultiReader<T>) {
        let queuedat: *mut QueueEntry<T>;
        unsafe {
            let alloc = allocate(capacity * mem::size_of::<T>(), mem::align_of::<T>());
            queuedat = mem::transmute(alloc);
            for i in 0..capacity as isize {
                let elem = &*queuedat.offset(i);
            }
        }
    }*/

    pub fn push_multi(&self, val: T) -> Result<(), T> {
        let mut transaction = self.head.load_transaction(Relaxed);

        // This esnures that metadata about the cursor group is in cache
        self.tail.prefetch_metadata();
        unsafe {
            loop {
                let tail_cache = self.tail_cache.load(Acquire);
                if transaction.matches_previous(tail_cache) {
                    if transaction.matches_previous(self.reload_tail_multi(tail_cache)) {
                        return Err(val);
                    }
                }
                // This isize conversion here helps performance on intel
                // since many (all?) 16-bit register ops incur a 3-cycle decoding penalty
                // The math works out anyways and the compiler can do it well
                let chead = transaction.get() as isize;
                let write_cell = &mut *self.data.offset(chead);
                let wrap_valid_tag = transaction.get_wraps().wrapping_add(1);
                match transaction.commit(1, Relaxed) {
                    Some(new_transaction) => transaction = new_transaction,
                    None => {
                        ptr::write(&mut write_cell.val, val);
                        write_cell.wraps.store(wrap_valid_tag, Release);
                        return Ok(());
                    }
                }
            }
        }
    }

    pub fn push_single(&self, val: T) -> Result<(), T> {
        let transaction = self.head.load_transaction(Relaxed);
        let chead = transaction.get() as isize;
        self.tail.prefetch_metadata();
        unsafe {
            if transaction.matches_previous(self.tail_cache.load(Relaxed)) {
                if transaction.matches_previous(self.reload_tail_single()) {
                    return Err(val);
                }
            }
            let write_cell = &mut *self.data.offset(chead);
            ptr::write(&mut write_cell.val, val);
            let wrap_valid_tag = transaction.get_wraps().wrapping_add(1);
            write_cell.wraps.store(wrap_valid_tag, Release);
            transaction.commit_direct(1, Relaxed);
            Ok(())
        }

        // Might consider letting the queue update the tail cache here preemptively
        // so it doesn't waste time before sending a message to do so
    }

    pub fn pop(&self, reader: &Reader) -> Option<T> {
        let mut ctail_attempt = reader.load_attempt(Relaxed);
        unsafe {
            loop {
                let ctail = ctail_attempt.get() as isize;
                let read_cell = &*self.data.offset(ctail);
                let wrap_valid_tag = ctail_attempt.get_wraps().wrapping_add(1);
                if read_cell.wraps.load(MAYBE_ACQUIRE) != wrap_valid_tag {
                    return None;
                }
                maybe_acquire_fence();
                let rval = ptr::read(&read_cell.val);
                match ctail_attempt.commit_attempt(1, Release) {
                    Some(new_attempt) => ctail_attempt = new_attempt,
                    None => return Some(rval),
                }
            }
        }
    }

    fn reload_tail_multi(&self, tail_cache: usize) -> usize {
        // This shows how far behind from head the reader is
        if let Some(max_diff_from_head) = self.tail.get_max_diff(self.head.load(Relaxed)) {
            let current_tail = self.head.get_previous(max_diff_from_head);
            match self.tail_cache.compare_exchange(tail_cache, current_tail, Relaxed, Acquire) {
                Ok(val) => val,
                Err(val) => val,
            }
        } else {
            self.tail_cache.load(Acquire)
        }
    }

    fn reload_tail_single(&self) -> usize {
        if let Some(max_diff_from_head) = self.tail.get_max_diff(self.head.load(Relaxed)) {
            let current_tail = self.head.get_previous(max_diff_from_head);
            self.tail_cache.store(current_tail, Relaxed);
            current_tail
        } else {
            // If this assert fires, memory has been corrupted
            assert!(false,
                    "The write head got ran over by consumers in isngle writer mode");
            0
        }
    }
}

impl<T> MultiWriter<T> {
    pub fn push(&self, val: T) -> Result<(), T> {
        match self.state.get() {
            QueueState::Single => self.queue.push_single(val),
            QueueState::Multi => {
                // This doesn't use the maybe_acquire framework since
                // it is so rarely acquire that it makes sense to incur
                // the rare extra cost of an acquire fence on architectures where it matters
                if self.queue.writers.load(Relaxed) == 1 {
                    fence(Acquire);
                    self.state.set(QueueState::Single);
                    self.queue.push_single(val)
                } else {
                    self.queue.push_multi(val)
                }
            }
        }
    }
}

impl<T> Clone for MultiWriter<T> {
    fn clone(&self) -> MultiWriter<T> {
        self.state.set(QueueState::Multi);
        let rval = MultiWriter {
            queue: self.queue.clone(),
            state: Cell::new(QueueState::Multi),
        };
        self.queue.writers.fetch_add(1, Release);
        rval
    }
}

impl<T> Drop for MultiWriter<T> {
    fn drop(&mut self) {
        self.queue.writers.fetch_sub(1, Release);
    }
}

unsafe impl<T> Sync for MultiQueue<T> {}
unsafe impl<T> Send for MultiQueue<T> {}
unsafe impl<T> Send for MultiWriter<T> {}
