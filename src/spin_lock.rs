use std::ops::{Deref, DerefMut};
use std::{
    cell::UnsafeCell,
    sync::atomic::{
        AtomicBool,
        Ordering::{Acquire, Release},
    },
};

pub struct SpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

unsafe impl<T> Sync for SpinLock<T> where T: Send {}

pub struct Guard<'a, T> {
    lock: &'a SpinLock<T>,
}

unsafe impl<T> Sync for Guard<'_, T> where T: Sync {}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> Guard<T> {
        while self.locked.swap(true, Acquire) {
            std::hint::spin_loop();
        }
        Guard { lock: self }
    }
}

impl<T> Deref for Guard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Release);
    }
}

#[cfg(test)]
mod test {
    use super::SpinLock;
    use std::thread;

    #[test]
    fn test() {
        let lock = SpinLock::new(vec![]);
        thread::scope(|s| {
            s.spawn(|| lock.lock().push(1));
            s.spawn(|| {
                let mut g = lock.lock();
                g.push(2);
                g.push(3);
            });
        });
        let g = lock.lock();
        assert!(*g == vec![1, 2, 3] || *g == vec![2, 3, 1]);
    }
}
