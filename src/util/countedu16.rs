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
