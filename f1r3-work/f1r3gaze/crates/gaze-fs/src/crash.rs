//! Crash enumeration over [`MemFs`]: a piece of work is run, crashed after
//! each of its operations, and then cut off from power in every way its
//! pending operations allow, and the state is checked each time.
//!
//! Every crashed state is a fork made with `MemFs::clone`, never a shared
//! `Arc::clone`: power cuts applied to one shared file system would
//! compound, so later cuts would find nothing pending and test nothing
//! (ledger S6).

use crate::MemFs;
use std::fmt;
use std::sync::Arc;

/// The crashes that left a state: one or two process crashes, each after
/// some of the work's operations, and perhaps a power cut after the last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crash {
    /// For each process crash: after how many operations, of how many.
    pub after: Vec<(usize, usize)>,
    /// The power cut, if there was one: which pending operations it kept.
    pub power: Option<Vec<bool>>,
}

impl fmt::Display for Crash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, (k, total)) in self.after.iter().enumerate() {
            match i {
                0 => write!(f, "a process crash after {k} of {total} operations")?,
                _ => write!(f, ", a restart and a crash after {k} of {total}")?,
            }
        }
        match &self.power {
            Some(keep) => write!(f, ", then a power cut keeping {keep:?}"),
            None => Ok(()),
        }
    }
}

/// Runs `jobs` (the crash points `0..=last`) on every core, each core
/// taking every `n`-th point so slow and fast points spread evenly, and
/// returns the sum of what `job` returns. A panic in a job ends the test
/// with its message, as it would on one thread.
fn on_every_core(last: usize, job: impl Fn(usize) -> usize + Sync) -> usize {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()).min(last + 1);
    std::thread::scope(|scope| {
        let job = &job;
        let workers: Vec<_> = (0..cores)
            .map(|first| scope.spawn(move || (first..=last).step_by(cores).map(job).sum::<usize>()))
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)))
            .sum()
    })
}

/// Runs `work` on a fork of `start` to count its operations. Then, for every
/// `k` from 0 to that count:
/// 1. runs it on a fresh fork that fails after `k` operations (a process
///    crash), and checks that state;
/// 2. checks every state a power cut could leave after that crash.
///
/// The crash points run on every core; each state is its own fork, so the
/// order of the checks does not matter. `work` gets the file system as an
/// `Arc`, so it can hand it to code that keeps one. Panics if the work fails
/// without a crash. Returns how many states were checked.
pub fn every_crash<T, E: std::fmt::Debug>(
    start: &MemFs,
    work: impl Fn(&Arc<MemFs>) -> Result<T, E> + Sync,
    check: impl Fn(&Crash, &MemFs) + Sync,
) -> usize {
    let total = count_ops(start, &work);
    on_every_core(total, |k| {
        let crashed = crash_after(start, &work, k);
        let after = vec![(k, total)];
        check(&Crash { after: after.clone(), power: None }, &crashed);
        let mut checked = 1;
        for keep in crashed.power_cuts() {
            let cut = MemFs::clone(&crashed);
            cut.power_cut(|index| keep[index]);
            check(&Crash { after: after.clone(), power: Some(keep) }, &cut);
            checked += 1;
        }
        checked
    })
}

/// Two crashes: for every `k1`, the work is crashed after `k1` operations;
/// a new process then runs `between` (the start-up barrier, say) and the
/// work again, crashed after each `k2` of its operations, and every power
/// cut after that second crash is checked. The first crash points run on
/// every core. Returns how many states were checked. Meant for small
/// shapes: the count grows as the product of the two runs' lengths and the
/// power cuts.
pub fn every_two_crashes<T, E: std::fmt::Debug>(
    start: &MemFs,
    between: impl Fn(&Arc<MemFs>) + Sync,
    work: impl Fn(&Arc<MemFs>) -> Result<T, E> + Sync,
    check: impl Fn(&Crash, &MemFs) + Sync,
) -> usize {
    let total = count_ops(start, &work);
    on_every_core(total, |k1| {
        let first = crash_after(start, &work, k1);
        let restart = |fs: &Arc<MemFs>| {
            between(fs);
            work(fs)
        };
        let second_total = count_ops(&first, &restart);
        let mut checked = 0;
        for k2 in 0..=second_total {
            let second = crash_after(&first, &restart, k2);
            let after = vec![(k1, total), (k2, second_total)];
            check(&Crash { after: after.clone(), power: None }, &second);
            checked += 1;
            for keep in second.power_cuts() {
                let cut = MemFs::clone(&second);
                cut.power_cut(|index| keep[index]);
                check(&Crash { after: after.clone(), power: Some(keep) }, &cut);
                checked += 1;
            }
        }
        checked
    })
}

/// How many operations `work` makes on a fork of `start`, run to the end.
fn count_ops<T, E: std::fmt::Debug>(start: &MemFs, work: &impl Fn(&Arc<MemFs>) -> Result<T, E>) -> usize {
    let whole = Arc::new(MemFs::clone(start));
    whole.reset_ops();
    if let Err(e) = work(&whole) {
        panic!("the work fails even without a crash: {e:?}");
    }
    whole.ops()
}

/// A fork of `start` on which `work` died after `k` operations, revived as a
/// new process would find it.
fn crash_after<T, E: std::fmt::Debug>(start: &MemFs, work: &impl Fn(&Arc<MemFs>) -> Result<T, E>, k: usize) -> MemFs {
    let crashed = Arc::new(MemFs::clone(start));
    crashed.fail_after(k);
    // The work fails from its (k+1)-th operation on: that is the crash.
    let _ = work(&crashed);
    crashed.revive();
    MemFs::clone(&crashed)
}
