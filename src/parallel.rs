//! A small work-sharing `map` over a slice using scoped threads.

use std::{
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

/// Upper bound on worker threads. Hashing is I/O-bound, so more threads rarely help
/// and would only compete with whatever else the machine is doing.
const MAX_WORKERS: usize = 4;

/// Applies `f` to every item on a pool of scoped threads.
///
/// Results are returned in the same order as `items`.
pub fn parallel_map<T, R, F>(items: &[T], f: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    // Use at most half the cores so the machine stays responsive.
    let workers = (thread::available_parallelism().map_or(1, |n| n.get()) / 2)
        .clamp(1, MAX_WORKERS)
        .min(items.len().max(1));
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::with_capacity(items.len()));

    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else { break };
                    let value = f(item);
                    results.lock().expect("worker panicked").push((index, value));
                }
            });
        }
    });

    let mut out = results.into_inner().expect("worker panicked");
    out.sort_unstable_by_key(|(index, _)| *index);
    out.into_iter().map(|(_, value)| value).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_order() {
        let input: Vec<u32> = (0..500).collect();
        let out = parallel_map(&input, |n| n * 2);
        assert_eq!(out, input.iter().map(|n| n * 2).collect::<Vec<_>>());
    }

    #[test]
    fn handles_empty_input() {
        let out: Vec<u8> = parallel_map(&Vec::<u8>::new(), |n| *n);
        assert!(out.is_empty());
    }
}
