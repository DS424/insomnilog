//! Tests for the lifecycle API: `start`, `shutdown`, `ShutdownGuard`.
//!
//! The library uses a process-wide `OnceLock<Backend>`; `cargo nextest run`
//! gives every test its own process so each one starts with a fresh global
//! (see Plan.md §5 "Global vs explicit Backend handle").

use std::time::Duration;

use insomnilog::{AlreadyStarted, BackendOptions, ShutdownGuard, shutdown, start};

#[test]
fn start_with_default_options_returns_guard() {
    let guard: ShutdownGuard =
        start(BackendOptions::default()).expect("first start should succeed");
    drop(guard);
}

#[test]
fn start_with_custom_options_succeeds() {
    let opts = BackendOptions {
        thread_name: "custom-backend".into(),
        idle_sleep: Duration::from_millis(1),
        ..BackendOptions::default()
    };
    let _guard = start(opts).expect("start with custom options should succeed");
}

/// Linux caps thread names at 15 characters via `pthread_setname_np`; "custom-backend" is 14.
#[test]
#[cfg(all(target_os = "linux", not(miri)))]
fn backend_thread_name_is_visible_in_os_thread_list() {
    let opts = BackendOptions {
        thread_name: "custom-backend".into(),
        ..BackendOptions::default()
    };
    let _guard = start(opts).expect("start should succeed");

    // The worker sets its name via pthread_setname_np from within itself, so
    // there is a small window after spawn() returns where the name is not yet
    // visible in /proc. Poll until it appears or a generous deadline expires.
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    let found = loop {
        let visible = std::fs::read_dir("/proc/self/task")
            .expect("/proc/self/task should be readable")
            .filter_map(|entry| std::fs::read_to_string(entry.ok()?.path().join("comm")).ok())
            .any(|s| s.trim() == "custom-backend");
        if visible || std::time::Instant::now() >= deadline {
            break visible;
        }
        std::thread::sleep(Duration::from_millis(10));
    };

    assert!(
        found,
        "expected a thread named 'custom-backend' to appear within 1s"
    );
}

#[test]
fn start_called_twice_returns_already_started() {
    let _guard = start(BackendOptions::default()).expect("first start should succeed");
    let result: Result<ShutdownGuard, AlreadyStarted> = start(BackendOptions::default());
    assert!(
        matches!(result, Err(AlreadyStarted)),
        "second start should return Err(AlreadyStarted)"
    );
}

#[test]
fn shutdown_without_start_is_a_noop() {
    shutdown();
    shutdown();
}

#[test]
fn shutdown_called_twice_after_start_is_idempotent() {
    let _guard = start(BackendOptions::default()).expect("start should succeed");
    shutdown();
    shutdown();
}

#[test]
fn manual_shutdown_then_guard_drop_does_not_panic() {
    let guard = start(BackendOptions::default()).expect("start should succeed");
    shutdown();
    drop(guard); // guard's Drop calls shutdown() again — must be a safe no-op
}

#[test]
fn guard_drop_alone_shuts_the_backend_down() {
    {
        let _guard = start(BackendOptions::default()).expect("start should succeed");
        // guard goes out of scope here, triggering shutdown via Drop
    }
    // After the guard dropped, calling shutdown() again must still be a no-op.
    shutdown();
}

#[test]
fn concurrent_start_only_one_thread_wins() {
    use std::sync::Barrier;
    use std::thread;

    const N: usize = 8;
    let barrier = std::sync::Arc::new(Barrier::new(N));
    #[expect(
        clippy::needless_collect,
        reason = "all threads must be spawned before any is joined; \
                  a lazy chain would deadlock on the Barrier"
    )]
    let handles: Vec<_> = (0..N)
        .map(|_| {
            let barrier = std::sync::Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                start(BackendOptions::default())
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let ok_count = results.iter().filter(|r| r.is_ok()).count();
    let err_count = results
        .iter()
        .filter(|r| matches!(r, Err(AlreadyStarted)))
        .count();
    assert_eq!(ok_count, 1, "exactly one start() must succeed");
    assert_eq!(err_count, N - 1, "all others must report AlreadyStarted");
}

#[test]
fn start_after_shutdown_still_returns_already_started() {
    // start() is conceptually one-shot per process. After shutdown, the
    // OnceLock is still occupied, so a second start must report AlreadyStarted
    // rather than silently spawning a fresh backend.
    let _guard = start(BackendOptions::default()).expect("first start should succeed");
    shutdown();
    let result = start(BackendOptions::default());
    assert!(
        matches!(result, Err(AlreadyStarted)),
        "start after shutdown should still report AlreadyStarted"
    );
}
