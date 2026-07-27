---
paths:
  - src/**/*.rs
  - tests/**/*.rs
---

# Tokio Best Practices

## Task Spawning

- `tokio::spawn` for async work that should run concurrently
- `spawn_blocking` for blocking I/O only (file system, synchronous libraries) — **not** for CPU-bound work (use Rayon instead)
- `spawn_blocking` tasks **cannot be cancelled** — keep them short and focused
- Avoid spawning tasks for trivial operations — just `.await` them inline

## Channel Selection

| Channel     | Pattern    | Use Case                                      |
|-------------|------------|-----------------------------------------------|
| `mpsc`      | Fan-in     | Multiple producers, single consumer            |
| `oneshot`   | One-shot   | Single response (e.g., request/reply)          |
| `broadcast` | Fan-out    | Multiple consumers, all receive every message  |
| `watch`     | Latest     | Single latest value, multiple observers        |

- For request/response: combine `mpsc` (send request) + `oneshot` (receive reply)
- Use `watch` for ViewModel updates from backend to frontend listeners
- `Receiver::recv_many(&mut buf, limit)` batch-drains an `mpsc` in one await — cancel-safe (a losing `select!` branch consumed nothing) and returns 0 only when the channel is closed *and* empty. Fits coalescing patterns (file-event batches, drop floods)
- `watch` consumer loops should be do-while shaped: process `rx.borrow_and_update()` first, *then* `await changed()` — handles the initial value and can't miss an update that landed between subscribe and first await

## Mutex & Locking

- **Never hold `MutexGuard` across `.await` points** — causes deadlocks with `tokio::sync::Mutex` and panics with `std::sync::Mutex` in multi-threaded runtime
- Prefer `std::sync::Mutex` over `tokio::sync::Mutex` — it's faster and sufficient when lock is not held across `.await`
- Use `tokio::sync::Mutex` only when you must hold the lock across an `.await`
- Drop RAII guards explicitly before `.await` points: `{ let guard = lock.lock(); /* use */ } // dropped here`

## `select!` Macro

- Hoist futures outside the `select!` loop — declaring inside recreates them each iteration
- Use `&mut pinned_future` to avoid consuming futures on each `select!` branch
- Beware: unselected branches are **cancelled** — ensure futures are cancel-safe or use `tokio::pin!`

## Graceful Shutdown

- Use `CancellationToken` from `tokio-util` to broadcast shutdown signals
- Use `TaskTracker` from `tokio-util` to wait for all spawned tasks to complete
- Pattern: detect shutdown signal → broadcast via token → await tracker completion
- `token.run_until_cancelled(fut)` returns `Some(output)`, or `None` once cancelled (dropping the future) — the concise alternative to a manual `select!` on `token.cancelled()`; completion wins a tie, and `run_until_cancelled_owned` consumes the token

## Common Pitfalls

- Blocking the runtime with synchronous I/O or CPU work (use `spawn_blocking` or Rayon)
- Holding locks across `.await` — restructure to lock, clone/extract data, unlock, then await
- Dropping a `JoinHandle` does **not** cancel the task — it detaches. Use `abort()` or `CancellationToken`
- Forgetting to `.await` a future — futures are lazy, nothing happens until polled
- Non-`Send` types (e.g. `Rc`, `RefCell`) held across `.await` — compiler error; use `Arc`/`Mutex` or drop before `.await`

## JoinSet

- Use `tokio::task::JoinSet` to spawn a collection of tasks and await them as they complete
- `join_set.join_next().await` returns the next completed task result (in completion order, not spawn order)
- `join_set.abort_all()` cancels all remaining tasks — good for fan-out patterns with early exit
- Unlike `join_all`, `JoinSet` lets you process results as they arrive without waiting for all tasks
- `JoinSet::join_all()` awaits everything at once and collects results in completion order — but it **panics on `JoinError`**; keep the `join_next()` loop when tasks may fail or get aborted

## Timeout & Cancellation

- `tokio::time::timeout(duration, future)` — wraps any future with a deadline; returns `Err(Elapsed)` on timeout
- For cancellation from outside a task: use `CancellationToken` (clone it into each task, check `.cancelled().await`)
- `oneshot::Receiver::closed()` — detect that the caller dropped the receiver and abort early

## Testing

- `#[tokio::test]` — runs a test on a single-threaded runtime by default
- `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` for multi-threaded tests
- `#[tokio::test(start_paused = true)]` — pauses the clock so `sleep`/`interval` resolve instantly; requires `test-util` feature
- `tokio::time::advance(duration)` — manually advance the paused clock inside a test
- Use `tokio_test::io::Builder` to mock `AsyncRead`/`AsyncWrite` in unit tests without real sockets

## Signals & OS Integration

- `tokio::signal::ctrl_c().await` — await Ctrl+C for graceful shutdown
- On Unix: `tokio::signal::unix::signal(SignalKind::terminate())` for SIGTERM
