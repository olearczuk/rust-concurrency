use std::{
    cell::UnsafeCell,
    marker::PhantomData,
    mem::MaybeUninit,
    sync::atomic::{
        AtomicBool,
        Ordering::{Acquire, Release},
    },
    thread::{self, Thread},
};

pub struct OneshotChannel<T> {
    message: UnsafeCell<MaybeUninit<T>>,
    ready: AtomicBool,
}

unsafe impl<T> Sync for OneshotChannel<T> where T: Send {}

pub struct Sender<'a, T> {
    channel: &'a OneshotChannel<T>,
    receiving_thread: Thread,
}

pub struct Receiver<'a, T> {
    channel: &'a OneshotChannel<T>,
    /// No Send because how thread parking is implemented
    _no_send: PhantomData<*const ()>,
}

impl<T> OneshotChannel<T> {
    pub const fn new() -> Self {
        Self {
            message: UnsafeCell::new(MaybeUninit::uninit()),
            ready: AtomicBool::new(false),
        }
    }

    pub fn split(&mut self) -> (Sender<T>, Receiver<T>) {
        // In case of channel being reused after Sender and Receiving being dropped
        *self = Self::new();
        (
            Sender {
                channel: self,
                receiving_thread: thread::current(),
            },
            Receiver {
                channel: self,
                _no_send: PhantomData,
            },
        )
    }
}

impl<T> Sender<'_, T> {
    pub fn send(self, message: T) {
        unsafe { (*self.channel.message.get()).write(message) };
        self.channel.ready.store(true, Release);
        self.receiving_thread.unpark();
    }
}

impl<T> Receiver<'_, T> {
    pub fn receive(self) -> T {
        while !self.channel.ready.swap(false, Acquire) {
            thread::park();
        }
        unsafe { (*self.channel.message.get()).assume_init_read() }
    }
}

impl<T> Drop for OneshotChannel<T> {
    fn drop(&mut self) {
        if *self.ready.get_mut() {
            unsafe { (*self.message.get()).assume_init_drop() }
        }
    }
}

#[cfg(test)]
mod test {
    use super::OneshotChannel;
    use std::thread;

    #[test]
    fn test() {
        let mut channel = OneshotChannel::new();
        thread::scope(|s| {
            let (sender, receiver) = channel.split();
            s.spawn(move || {
                sender.send("test");
            });
            assert_eq!(receiver.receive(), "test");
        })
    }
}
