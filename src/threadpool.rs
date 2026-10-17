//! Thread pool for running proxy resolution jobs.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

const MAX_THREADS: usize = 3;
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

type Job = Box<dyn FnOnce() + Send + 'static>;

struct State {
    queue: VecDeque<Job>,
    threads: usize,
    idle: usize,
}

pub(crate) struct ThreadPool {
    state: Mutex<State>,
    wakeup: Condvar,
    max_threads: usize,
}

/// Thread pool shared by all proxy resolvers.
pub(crate) static POOL: ThreadPool = ThreadPool::new(MAX_THREADS);

#[cfg(target_os = "macos")]
#[link(name = "objc")]
unsafe extern "C" {
    fn objc_autoreleasePoolPush() -> *mut std::ffi::c_void;
    fn objc_autoreleasePoolPop(pool: *mut std::ffi::c_void);
}

// Run a job, draining any autoreleased objects it creates
fn run_job(job: Job) {
    // The implicit autorelease pool of a thread on macOS doesn't drain until the thread exits
    #[cfg(target_os = "macos")]
    // SAFETY: The pool is pushed and popped on the same thread.
    let pool = unsafe { objc_autoreleasePoolPush() };

    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_err() {
        log::error!("Thread pool job panicked");
    }

    #[cfg(target_os = "macos")]
    // SAFETY: The pool was pushed above on this thread.
    unsafe {
        objc_autoreleasePoolPop(pool)
    };
}

impl ThreadPool {
    const fn new(max_threads: usize) -> Self {
        ThreadPool {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                threads: 0,
                idle: 0,
            }),
            wakeup: Condvar::new(),
            max_threads,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds a job to the queue, starting a new thread if all threads are busy.
    pub(crate) fn enqueue(&'static self, job: impl FnOnce() + Send + 'static) -> bool {
        let mut state = self.lock();
        state.queue.push_back(Box::new(job));
        if state.idle == 0 && state.threads < self.max_threads {
            let spawned = thread::Builder::new()
                .name("proxyres-worker".to_string())
                .spawn(move || self.work());
            match spawned {
                Ok(_) => state.threads += 1,
                Err(error) => {
                    log::error!("Unable to create thread ({error})");
                    // Without any threads the job would never run
                    if state.threads == 0 {
                        state.queue.pop_back();
                        return false;
                    }
                }
            }
        }
        drop(state);
        self.wakeup.notify_one();
        true
    }

    fn work(&'static self) {
        let mut state = self.lock();
        loop {
            if let Some(job) = state.queue.pop_front() {
                drop(state);
                run_job(job);
                state = self.lock();
                continue;
            }
            // Exit the thread once it has been idle for a while
            state.idle += 1;
            let (guard, wait) = self
                .wakeup
                .wait_timeout(state, IDLE_TIMEOUT)
                .unwrap_or_else(PoisonError::into_inner);
            state = guard;
            state.idle -= 1;
            if wait.timed_out() && state.queue.is_empty() {
                state.threads -= 1;
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    #[test]
    fn runs_all_jobs() {
        static TEST_POOL: ThreadPool = ThreadPool::new(MAX_THREADS);
        let count = Arc::new(AtomicUsize::new(0));
        let (sender, receiver) = mpsc::channel();
        for _ in 0..20 {
            let count = count.clone();
            let sender = sender.clone();
            assert!(TEST_POOL.enqueue(move || {
                count.fetch_add(1, Ordering::SeqCst);
                sender.send(()).unwrap();
            }));
        }
        for _ in 0..20 {
            receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        assert_eq!(count.load(Ordering::SeqCst), 20);
        assert!(TEST_POOL.lock().threads <= MAX_THREADS);
    }

    #[test]
    fn survives_panicking_job() {
        static TEST_POOL: ThreadPool = ThreadPool::new(1);
        let (sender, receiver) = mpsc::channel();
        TEST_POOL.enqueue(|| panic!("job panic"));
        TEST_POOL.enqueue(move || sender.send(()).unwrap());
        receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    }
}
