use super::mutex::MutexGuard;
use atomic_wait::{wait, wake_all, wake_one};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering::Relaxed};

pub struct Condvar {
    counter: AtomicU32,
    waiters_count: AtomicUsize,
}

impl Condvar {
    pub const fn new() -> Self {
        Self {
            counter: AtomicU32::new(0),
            waiters_count: AtomicUsize::new(0),
        }
    }

    pub fn notify_one(&self) {
        if self.waiters_count.load(Relaxed) != 0 {
            self.counter.fetch_add(1, Relaxed);
            wake_one(&self.counter);
        }
    }

    pub fn notify_all(&self) {
        if self.waiters_count.load(Relaxed) != 0 {
            self.counter.fetch_add(1, Relaxed);
            wake_all(&self.counter);
        }
    }

    pub fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
        self.waiters_count.fetch_add(1, Relaxed);

        let counter = self.counter.load(Relaxed);

        let mutex = guard.mutex;
        drop(guard);

        wait(&self.counter, counter);

        self.waiters_count.fetch_sub(1, Relaxed);

        mutex.lock()
    }
}

#[cfg(test)]
mod test {
    use super::super::mutex::Mutex;
    use super::Condvar;
    use std::{thread, time::Duration};

    #[test]
    fn test() {
        let mutex = Mutex::new(0);
        let condvar = Condvar::new();

        let mut wakeups = 0;

        thread::scope(|s| {
            s.spawn(|| {
                thread::sleep(Duration::from_secs(1));
                *mutex.lock() = 123;
                condvar.notify_one();
            });

            let mut m = mutex.lock();
            while *m < 100 {
                m = condvar.wait(m);
                wakeups += 1;
            }

            assert_eq!(*m, 123);
        });

        assert!(wakeups < 10);
    }
}
