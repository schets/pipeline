use std::sync::atomic::{AtomicUsize, Ordering};

pub struct CountedU16 {
    val: AtomicUsize,
    wrap: usize,
}

pub struct Transaction<'a> {
    ptr: &'a AtomicUsize,
    loaded_vals: usize,
    wrap: usize,
    lord: Ordering,
}

impl CountedU16 {
    pub fn new(val: u16, wrap: u16) -> CountedU16 {
        CountedU16 {
            val: AtomicUsize::new(val as usize),
            wrap: wrap as usize,
        }
    }

    pub fn from_usize(val: usize, wrap: u16) -> CountedU16 {
        CountedU16 {
            val: AtomicUsize::new(val),
            wrap: wrap as usize,
        }
    }

    pub fn wrap_at(&self) -> u16 {
        self.wrap as u16
    }

    #[inline(always)]
    pub fn load(&self, ord: Ordering) -> u16 {
        self.val.load(ord) as u16
    }

    #[inline(always)]
    pub fn load_raw(&self, ord: Ordering) -> usize {
        self.val.load(ord)
    }

    #[inline(always)]
    pub fn load_wraps(&self, ord: Ordering) -> usize {
        self.val.load(ord) >> 16
    }

    #[inline(always)]
    pub fn load_count(&self, ord: Ordering) -> usize {
        let val = self.val.load(ord);
        let lower_half = (val as u16) as usize;
        let upper_half = val >> 16;
        lower_half + self.wrap * upper_half
    }

    #[inline(always)]
    pub fn load_transaction(&self, ord: Ordering) -> Transaction {
        Transaction {
            ptr: &self.val,
            loaded_vals: self.val.load(ord),
            lord: ord,
            wrap: self.wrap,
        }
    }

    #[inline(always)]
    pub fn get_previous(&self, by: u16) -> usize {
        let vals = self.val.load(Ordering::Relaxed);
        let lower_half = vals as u16;
        let upper_half = vals & !(::std::u16::MAX as usize);
        if by <= lower_half {
            ((lower_half - by) as usize) + upper_half
        } else {
            let extra = (by - lower_half) as usize;
            (self.wrap - extra) + (upper_half.wrapping_sub(1 << 16))
        }
    }
}

impl<'a> Transaction<'a> {
    #[inline(always)]
    pub fn get(&self) -> u16 {
        self.loaded_vals as u16
    }

    #[inline(always)]
    pub fn get_wraps(&self) -> usize {
        self.loaded_vals >> 16
    }

    /// Returns true is the usize passed matches the value
    /// held by the transaction
    #[inline(always)]
    pub fn matches(&self, val: usize) -> bool {
        self.loaded_vals == val
    }

    /// Returns true if the values passed in matches the previous wrap-around of the Transaction
    #[inline(always)]
    pub fn matches_previous(&self, val: usize) -> bool {
        self.loaded_vals.wrapping_sub(1 << 16) == val
    }

    pub fn commit(self, by: u16, ord: Ordering) -> Option<Transaction<'a>> {
        let wrap = self.wrap;
        let bottom = self.loaded_vals as u16;
        let mut next = bottom.wrapping_add(by) as usize;
        let mut upper_half = self.loaded_vals;
        if next >= wrap {
            next -= wrap;
            upper_half = upper_half.wrapping_add(1 << 16);
        }
        let store_val = (next as usize) | (upper_half & !(::std::u16::MAX as usize));
        match self.ptr.compare_exchange_weak(self.loaded_vals, store_val, ord, self.lord) {
            Ok(_) => None,
            Err(cval) => {
                Some(Transaction {
                    ptr: self.ptr,
                    loaded_vals: cval,
                    lord: self.lord,
                    wrap: self.wrap,
                })
            }
        }
    }

    pub fn commit_direct(self, by: u16, ord: Ordering) {
        let wrap = self.wrap;
        let mut next = by.wrapping_add(self.loaded_vals as u16) as usize;
        let mut upper_half = self.loaded_vals;
        if next >= wrap {
            next -= wrap;
            upper_half = upper_half.wrapping_add(1 << 16);
        }
        let store_val = next | (upper_half & !(::std::u16::MAX as usize));
        self.ptr.store(store_val, ord);
    }
}

unsafe impl Send for CountedU16 {}
unsafe impl Sync for CountedU16 {}

#[cfg(test)]
mod tests {
    use super::*;

    extern crate crossbeam;
    use self::crossbeam::scope;

    use std::sync::atomic::Ordering::*;

    fn test_incr_param(wrap_size: u16, goaround: usize) {
        let mycounted = CountedU16::new(0, wrap_size);
        for j in 0..goaround {
            for i in 0..wrap_size as usize {
                let trans = mycounted.load_transaction(Relaxed);
                assert_eq!(i, mycounted.load(Relaxed) as usize);
                assert_eq!(j, mycounted.load_wraps(Relaxed));
                assert_eq!(i + (j * wrap_size as usize), mycounted.load_count(Relaxed));
                assert_eq!(i, trans.get() as usize);
                assert_eq!(j, trans.get_wraps());
                trans.commit_direct(1, Release);
            }
        }
        // Is wrap_size * goaround % wrap_size, so trivially zero
        assert_eq!(0, mycounted.load(Relaxed));
        assert_eq!(goaround, mycounted.load_wraps(Relaxed));
        assert_eq!(wrap_size as usize * goaround, mycounted.load_count(Relaxed));
    }

    fn test_incr_param_threaded(wrap_size: u16, goaround: usize, nthread: usize) {
        let mycounted = CountedU16::new(0, wrap_size);
        scope(|scope| for _ in 0..nthread {
            scope.spawn(|| for j in 0..goaround {
                for i in 0..wrap_size {
                    let mut trans = mycounted.load_transaction(Relaxed);
                    loop {
                        match trans.commit(1, Release) {
                            Some(new_t) => trans = new_t,
                            None => break,
                        }
                    }
                }
            });
        });
        assert_eq!(0, mycounted.load(Relaxed));
        assert_eq!(goaround * nthread, mycounted.load_wraps(Relaxed));
        assert_eq!(wrap_size as usize * goaround * nthread,
                   mycounted.load_count(Relaxed));
    }

    #[test]
    fn test_small() {
        test_incr_param(10, 100);
    }

    #[test]
    fn test_tiny() {
        test_incr_param(1, 100)
    }

    #[test]
    fn test_wrapu16() {
        test_incr_param(::std::u16::MAX, 2)
    }

    #[test]
    fn test_small_mt() {
        test_incr_param_threaded(10, 1000, 2)
    }

    #[test]
    fn test_tiny_mt() {
        test_incr_param_threaded(1, 10000, 2)
    }

    #[test]
    fn test_wrapu16_mt() {
        test_incr_param_threaded(::std::u16::MAX, 2, 10)
    }

    #[test]
    fn test_transaction_fail() {
        let mycounted = CountedU16::new(0, 10);
        let trans = mycounted.load_transaction(Relaxed);
        let trans2 = mycounted.load_transaction(Relaxed);
        trans2.commit_direct(1, Relaxed);
        trans.commit(1, Relaxed).unwrap();
    }
}
