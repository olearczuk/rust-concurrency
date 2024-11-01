use std::{
    cell::UnsafeCell,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicU32, Ordering::*},
};

use atomic_wait::{wait, wake_all, wake_one};

pub struct RwLock<T> {
    /// Number of read locks time two, plus one if there's a writer waiting.
    /// u32::MAX if locked by a writer.
    state: AtomicU32,
    /// Incremented to wake up writers.
    write_wake_counter: AtomicU32,
    value: UnsafeCell<T>,
}

unsafe impl<T> Sync for RwLock<T> where T: Send + Sync {}

impl<T> RwLock<T> {
    pub const fn new(data: T) -> Self {
        Self {
            state: AtomicU32::new(0),
            write_wake_counter: AtomicU32::new(0),
            value: UnsafeCell::new(data),
        }
    }

    pub fn read(&self) -> ReadGuard<T> {
        let mut state = self.state.load(Relaxed);
        loop {
            // No active / pending writers, okay to lock
            if state % 2 == 0 {
                assert!(state < u32::MAX - 2, "too many readers");
                match self
                    .state
                    .compare_exchange_weak(state, state + 2, Acquire, Relaxed)
                {
                    Ok(_) => return ReadGuard { rwlock: self },
                    Err(e) => state = e,
                }
            }

            // Pending writer, wait so writers are not starved
            if state % 2 == 1 {
                wait(&self.state, state);
                state = self.state.load(Relaxed);
            }
        }
    }

    pub fn write(&self) -> WriteGuard<T> {
        let mut state = self.state.load(Relaxed);
        loop {
            // No readers, try to lock
            if state <= 1 {
                match self
                    .state
                    .compare_exchange(state, u32::MAX, Acquire, Relaxed)
                {
                    Ok(_) => return WriteGuard { rwlock: self },
                    Err(e) => {
                        state = e;
                        continue;
                    }
                }
            }

            // Inform the readers about waiting writer
            // u32::MAX is odd so this won't be executed when another writer locks it
            if state % 2 == 0 {
                if let Err(e) = self
                    .state
                    .compare_exchange(state, state + 1, Relaxed, Relaxed)
                {
                    state = e;
                    continue;
                }
            }

            // Locked by someone else, need to wait
            let w = self.write_wake_counter.load(Acquire);
            state = self.state.load(Relaxed);
            if state >= 2 {
                wait(&self.write_wake_counter, w);
                state = self.state.load(Relaxed);
            }
        }
    }
}

pub struct ReadGuard<'a, T> {
    rwlock: &'a RwLock<T>,
}

impl<T> Deref for ReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.rwlock.value.get() }
    }
}

impl<T> Drop for ReadGuard<'_, T> {
    fn drop(&mut self) {
        if self.rwlock.state.fetch_sub(2, Release) == 3 {
            self.rwlock.write_wake_counter.fetch_add(1, Release);
            wake_one(&self.rwlock.write_wake_counter);
        }
    }
}

pub struct WriteGuard<'a, T> {
    rwlock: &'a RwLock<T>,
}

impl<T> Deref for WriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.rwlock.value.get() }
    }
}

impl<T> DerefMut for WriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.rwlock.value.get() }
    }
}

impl<T> Drop for WriteGuard<'_, T> {
    fn drop(&mut self) {
        self.rwlock.state.store(0, Release);
        self.rwlock.write_wake_counter.fetch_add(1, Release);

        wake_one(&self.rwlock.write_wake_counter);
        wake_all(&self.rwlock.state);
    }
}

#[cfg(test)]
mod test {
    use std::thread;

    use super::RwLock;

    #[test]
    fn test() {
        let writers = 2;
        let increase_per_writer = 100;
        let rwlock = RwLock::new(0);

        thread::scope(|s| {
            let reader = || {
                let mut prev_val = -1;
                loop {
                    let val = rwlock.read();

                    assert!(*val <= writers * increase_per_writer);
                    assert!(prev_val <= *val);

                    prev_val = *val;
                    if prev_val == writers * increase_per_writer {
                        break;
                    }
                }
            };

            s.spawn(reader);
            s.spawn(reader);
            s.spawn(reader);

            for _ in 0..writers {
                s.spawn(|| {
                    for _ in 0..increase_per_writer {
                        *rwlock.write() += 1;
                    }
                });
            }
        });

        assert_eq!(*rwlock.read(), 200);
    }
}
