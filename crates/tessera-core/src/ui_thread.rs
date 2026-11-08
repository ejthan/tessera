// SPDX-License-Identifier: GPL-3.0-or-later

//! Debug guard for NF4: blocking calls stay off the UI thread. No test harness marks a thread,
//! so GPUI tests, where background work shares a thread, cannot trip it.

use std::cell::Cell;

thread_local! {
    static MARKED: Cell<bool> = const { Cell::new(false) };
}

/// Marks this thread as the UI thread. Called once, inside `application().run`.
pub fn mark() {
    MARKED.set(true);
}

/// Debug builds: panics when called on the marked thread. Release: nothing.
pub fn assert_off(what: &'static str) {
    debug_assert!(!MARKED.get(), "blocking call on the UI thread: {what}");
}

#[cfg(test)]
mod tests {
    use std::panic::catch_unwind;

    use super::*;

    #[test]
    fn a_marked_thread_trips_only_in_debug() {
        std::thread::spawn(|| {
            mark();
            let tripped = catch_unwind(|| assert_off("x")).is_err();
            assert_eq!(tripped, cfg!(debug_assertions));
        })
        .join()
        .expect("the check held");
    }

    #[test]
    fn an_unmarked_thread_passes() {
        std::thread::spawn(|| assert_off("x"))
            .join()
            .expect("no mark, no panic");
    }
}
