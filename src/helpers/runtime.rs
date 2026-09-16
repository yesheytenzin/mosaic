// SPDX-License-Identifier: GPL-3.0-or-later

//! Bridge from synchronous code to the async helpers.
//!
//! Parts of the CLI are synchronous (init, upgrade, status) but call async
//! helpers such as the HTTP client and the D-Bus proxies. Building a fresh
//! runtime for that panics when the async entry point is already driving the
//! current thread, so go through here instead.

/// Run a future to completion on the current thread.
///
/// When a tokio runtime is already running (the `#[tokio::main]` entry point),
/// the future is driven by a plain executor from inside `block_in_place`.
/// `block_in_place` hands the current worker's tasks to another worker, which
/// keeps the IO and timer drivers running, so tokio-dependent futures still
/// make progress. Calling `Handle::block_on` here instead would panic, because
/// it tries to re-enter the runtime that is already entered.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(|| futures_executor::block_on(future)),
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build a tokio runtime")
            .block_on(future),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_on_works_without_a_runtime() {
        assert_eq!(block_on(async { 40 + 2 }), 42);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn block_on_works_inside_a_runtime() {
        // This used to panic with "Cannot start a runtime from within a runtime".
        assert_eq!(block_on(async { 6 * 7 }), 42);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn block_on_drives_tokio_timers_from_inside_a_runtime() {
        // Proves the runtime's driver keeps running while we block, which is
        // what lets reqwest and other async futures complete here.
        let value = block_on(async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            "timer fired"
        });
        assert_eq!(value, "timer fired");
    }
}
