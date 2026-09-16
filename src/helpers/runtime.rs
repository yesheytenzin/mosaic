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
/// this parks the worker via `block_in_place`. That requires the multi-thread
/// flavour, which the entry point uses.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(future)),
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

    #[tokio::test(flavor = "multi_thread")]
    async fn block_on_works_inside_a_runtime() {
        // This used to panic with "Cannot start a runtime from within a runtime".
        assert_eq!(block_on(async { 6 * 7 }), 42);
    }
}
