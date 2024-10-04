use std::{
    cell::UnsafeCell,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicU32, AtomicU8, Ordering::*},
};

use atomic_wait::{wait, wake_one};

pub struct Mutex<T> {
    // 0: unlocked
    // 1: locked, no waiting threads
    // 2: locked, some waiting threads
    state: AtomicU32,
    data: UnsafeCell<T>,
}

unsafe impl<T> Sync for Mutex<T> where T: Send {}

impl<T> Mutex<T> {
    pub const fn new(data: T) -> Self {
        return Mutex {
            state: AtomicU32::new(0),
            data: UnsafeCell::new(data),
        };
    }

    pub fn lock(&self) -> MutexGuard<T> {
        if self.state.compare_exchange(0, 1, Acquire, Relaxed).is_err() {
            lock_contended(&self.state);
        }
        MutexGuard { mutex: self }
    }
}

fn lock_contended(state: &AtomicU32) {
    let mut spin_count = 0;
    while state.load(Relaxed) == 1 && spin_count < 100 {
        spin_count += 1;
        std::hint::spin_loop();
    }

    if state.compare_exchange(0, 1, Acquire, Relaxed).is_ok() {
        return;
    }

    while state.swap(2, Acquire) != 0 {
        wait(state, 2);
    }
}

pub struct MutexGuard<'a, T> {
    pub mutex: &'a Mutex<T>,
}

unsafe impl<T> Sync for MutexGuard<'_, T> where T: Sync {}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        if self.mutex.state.swap(0, Release) == 2 {
            wake_one(&self.mutex.state);
        }
    }
}

#[cfg(test)]
mod test {
    use super::Mutex;
    use std::thread;

    #[test]
    fn test() {
        let mutex = Mutex::new(vec![]);
        thread::scope(|s| {
            s.spawn(|| mutex.lock().push(1));
            s.spawn(|| {
                let mut g = mutex.lock();
                g.push(2);
                g.push(3);
            });
        });
        let g = mutex.lock();
        assert!(*g == vec![1, 2, 3] || *g == vec![2, 3, 1]);
    }
}
