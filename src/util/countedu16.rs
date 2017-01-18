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
    pub fn new(val: u16, wrap: usize) -> CountedU16 {
        CountedU16 {
            val: AtomicUsize::new(val as usize),
            wrap: wrap,
        }
    }

    #[inline(always)]
    pub fn load(&self, ord: Ordering) -> u16 {
        self.val.load(ord) as u16
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
        }
        else {
            let extra = (by - lower_half) as usize;
            (self.wrap - extra) + (upper_half - (1 << 16))
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
        (self.loaded_vals - (1 << 16)) == val
    }

    pub fn commit(self, by: u16, ord: Ordering) -> Option<Transaction<'a>> {
        let wrap = self.wrap;
        let mut next = (by as usize) + wrap;
        let mut upper_half = self.loaded_vals;
        if next >= wrap {
            next -= wrap;
            upper_half = upper_half.wrapping_add(1 << 16);
        }
        let store_val = (next as usize) | (upper_half & !(::std::u16::MAX as usize));
        match self.ptr.compare_exchange_weak(self.loaded_vals, store_val, ord, self.lord) {
            Ok(_) => None,
            Err(cval) => Some(Transaction {
                ptr: self.ptr,
                loaded_vals: cval,
                lord: self.lord,
                wrap: self.wrap,
            })
        }
    }

    pub fn commit_direct(self, by: u16, ord: Ordering) {
        let wrap = self.wrap;
        let mut next = (by as usize) + wrap;
        let mut upper_half = self.loaded_vals;
        if next >= wrap {
            next -= wrap;
            upper_half = upper_half.wrapping_add(1 << 16);
        }
        let store_val = (next as usize) | (upper_half & !(::std::u16::MAX as usize));
        self.ptr.store(store_val, ord);
    }
}

unsafe impl Send for CountedU16 {}
unsafe impl Sync for CountedU16 {}
