use std::{
    cell::UnsafeCell,
    mem::ManuallyDrop,
    ops::Deref,
    ptr::NonNull,
    sync::atomic::{
        fence, AtomicUsize,
        Ordering::{Acquire, Relaxed, Release},
    },
};

const WEAK_COUNT_LOCKED_VAL: usize = usize::MAX;
const COUNT_LIMIT: usize = usize::MAX / 2;

pub struct Arc<T> {
    ptr: NonNull<ArcData<T>>,
}

unsafe impl<T: Sync + Send> Send for Arc<T> {}
unsafe impl<T: Sync + Send> Sync for Arc<T> {}

pub struct Weak<T> {
    ptr: NonNull<ArcData<T>>,
}

unsafe impl<T: Sync + Send> Send for Weak<T> {}
unsafe impl<T: Sync + Send> Sync for Weak<T> {}

struct ArcData<T> {
    /// Number of `Arc`s
    strong: AtomicUsize,
    /// Number of `Weak`s, plus one if there is any `Arc`
    weak: AtomicUsize,
    /// Dropped if there are no `Arc`s pointers left.
    data: UnsafeCell<ManuallyDrop<T>>,
}

impl<T> Arc<T> {
    pub fn new(data: T) -> Arc<T> {
        Arc {
            ptr: NonNull::from(Box::leak(Box::new(ArcData {
                strong: AtomicUsize::new(1),
                weak: AtomicUsize::new(1),
                data: UnsafeCell::new(ManuallyDrop::new(data)),
            }))),
        }
    }

    pub fn get_mut(&self) -> Option<&mut T> {
        // Lock weak pointer count if we are the sole weak pointer holder.
        // This prevents any `Arc` from getting downgraded to `Weak`.
        //
        // The acquire matches `Weak::drop`decrement.
        // This guarantees visiblity of any writes to strong
        // (in particular `Weak::upgrade`) prior to weak decrements (via `Weak::drop`).
        // If the upgraded weak ref was never dropped, the CAS here will fail anyway.
        if self
            .data()
            .weak
            .compare_exchange(1, WEAK_COUNT_LOCKED_VAL, Acquire, Relaxed)
            .is_err()
        {
            return None;
        }

        // `Acquire` to synchronise with the decrement of the strong counter in `Arc::drop`.
        // This way we ensure there are no other accesses.
        let is_unique = self.data().strong.load(Acquire) == 1;

        // Release the weak pointer count lock.
        // Synchronises with read in `Arc::downgrade`, to make sure any changes to
        // strong count that come after `downgrade` don't change the uniqueness check above.
        self.data().weak.store(1, Release);
        if !is_unique {
            return None;
        }

        unsafe { Some(&mut *self.data().data.get()) }
    }

    pub fn downgrade(&self) -> Weak<T> {
        let mut n = self.data().strong.load(Relaxed);
        loop {
            // Check whether weak count is locked.
            if n == WEAK_COUNT_LOCKED_VAL {
                std::hint::spin_loop();
                n = self.data().strong.load(Relaxed);
                continue;
            }
            assert!(n <= COUNT_LIMIT);

            // Acquire synchronises with `Arc::get_mut` release store.
            match self
                .data()
                .weak
                .compare_exchange_weak(n, n + 1, Acquire, Relaxed)
            {
                Err(e) => n = e,
                Ok(_) => return Weak { ptr: self.ptr },
            }
        }
    }

    fn data(&self) -> &ArcData<T> {
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> Deref for Arc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // Safety: since there's an Arc, the data exists and can be shared.
        unsafe { &*self.data().data.get() }
    }
}

impl<T> Clone for Arc<T> {
    fn clone(&self) -> Self {
        if (self.data().strong.fetch_add(1, Relaxed)) >= COUNT_LIMIT {
            std::process::abort();
        }
        Arc { ptr: self.ptr }
    }
}

impl<T> Drop for Arc<T> {
    fn drop(&mut self) {
        if self.data().strong.fetch_sub(1, Release) == 1 {
            fence(Acquire);
            // Safety: Strong counter is zero, nothing can access the data anymore.
            unsafe {
                ManuallyDrop::drop(&mut *self.data().data.get());
            }
            // No `Arc`s left, drop the implicit weak pointer that represents all `Arc`s.
            drop(Weak { ptr: self.ptr });
        }
    }
}

impl<T> Weak<T> {
    pub fn upgrade(&self) -> Option<Arc<T>> {
        let mut n = self.data().strong.load(Relaxed);

        loop {
            if n == 0 {
                return None;
            }
            assert!(n <= COUNT_LIMIT);

            match self
                .data()
                .strong
                .compare_exchange_weak(n, n + 1, Relaxed, Relaxed)
            {
                Err(e) => n = e,
                Ok(_) => return Some(Arc { ptr: self.ptr }),
            }
        }
    }

    fn data(&self) -> &ArcData<T> {
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> Clone for Weak<T> {
    fn clone(&self) -> Self {
        if (self.data().weak.fetch_add(1, Relaxed)) >= COUNT_LIMIT {
            std::process::abort();
        }
        Weak { ptr: self.ptr }
    }
}

impl<T> Drop for Weak<T> {
    fn drop(&mut self) {
        // Release synchronises with `Arc::get_mut` acquire load.
        if self.data().weak.fetch_sub(1, Release) == 1 {
            fence(Acquire);
            // Safety: Weak counter is zero, nothing can access the pointer anymore.
            unsafe {
                drop(Box::from_raw(self.ptr.as_ptr()));
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::{cell::RefCell, thread::spawn};

    static DETECT_DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

    struct DetectDrop;
    unsafe impl Send for DetectDrop {}
    unsafe impl Sync for DetectDrop {}

    impl Drop for DetectDrop {
        fn drop(&mut self) {
            DETECT_DROP_COUNT.fetch_add(1, Relaxed);
        }
    }

    fn check_counters(
        ptr: NonNull<ArcData<(&str, DetectDrop)>>,
        exp_strong: usize,
        exp_weak: usize,
    ) {
        assert_eq!(unsafe { ptr.as_ref().strong.load(Relaxed) }, exp_strong);
        assert_eq!(unsafe { ptr.as_ref().weak.load(Relaxed) }, exp_weak);
    }

    #[test]
    fn test_various() {
        DETECT_DROP_COUNT.store(0, Relaxed);

        let strong = Arc::new(("hello", DetectDrop));
        assert!(strong.get_mut().is_some());

        let weak1 = strong.downgrade();
        let weak2 = strong.downgrade();

        assert!(strong.get_mut().is_none());
        check_counters(weak1.ptr, 1, 3);

        let t = spawn(move || {
            let temp_strong = weak1.upgrade().unwrap();
            drop(weak1);

            assert_eq!(temp_strong.0, "hello");
            // strong, weak2 and temp_strong
            check_counters(temp_strong.ptr, 2, 2);
        });
        assert_eq!(strong.0, "hello");
        t.join().unwrap();
        // strong and weak2
        check_counters(weak2.ptr, 1, 2);

        assert_eq!(DETECT_DROP_COUNT.load(Relaxed), 0);
        assert!(weak2.upgrade().is_some());

        drop(weak2);
        assert_eq!(DETECT_DROP_COUNT.load(Relaxed), 0);

        // strong
        check_counters(strong.ptr, 1, 1);
        assert!(strong.get_mut().is_some());

        let weak3 = strong.downgrade();
        // strong, weak3
        check_counters(strong.ptr, 1, 2);

        drop(strong);
        assert_eq!(DETECT_DROP_COUNT.load(Relaxed), 1);

        // weak3
        check_counters(weak3.ptr, 0, 1);
        assert!(weak3.upgrade().is_none());
    }

    static A_B_DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

    struct A {
        b: Option<Arc<RefCell<B>>>,
    }

    impl Drop for A {
        fn drop(&mut self) {
            A_B_DROP_COUNT.fetch_add(1, Relaxed);
        }
    }

    struct B {
        _a: Weak<RefCell<A>>,
    }

    impl Drop for B {
        fn drop(&mut self) {
            A_B_DROP_COUNT.fetch_add(1, Relaxed);
        }
    }

    #[test]
    fn test_arc_weak_cycle() {
        A_B_DROP_COUNT.store(0, Relaxed);

        {
            let a = Arc::new(RefCell::new(A { b: None }));
            let b = Arc::new(RefCell::new(B { _a: a.downgrade() }));
            (*a).borrow_mut().b = Some(b);
        }

        assert_eq!(A_B_DROP_COUNT.load(Relaxed), 2);
    }
}
