---
paths:
  - src/media/**/*.rs
  - src/tasks/**/*.rs
  - src/library/**/*.rs
---

# Rayon Best Practices

## When to Use

- **CPU-bound work** → Rayon (`par_iter`, `par_sort`, parallel map/reduce)
- **I/O-bound work** → Tokio (async, non-blocking)
- **Mixed** → Bridge with `rayon::spawn` + `tokio::sync::oneshot` channel

## Parallel Iteration

- Replace `.iter()` with `.par_iter()` for data-parallel operations
- Chain operations freely — Rayon parallelizes the entire pipeline, not just individual steps
- `par_iter().map().filter().collect()` works like sequential but parallel
- Use `par_sort_unstable()` over `par_sort()` when element order among equals doesn't matter — faster

## Thread Pool

- Default thread count = number of logical CPUs (usually optimal)
- Custom pools via `ThreadPoolBuilder::new().num_threads(n).build()` — use `pool.install(|| ...)` to run work on it
- A single global pool is usually sufficient — create custom pools only for isolation

## Performance Considerations

- **Don't parallelize small collections** — overhead dominates below ~1000 trivial items
- Default work-splitting is usually optimal — only tune with `with_min_len()`/`with_max_len()` after benchmarking
- Avoid locks inside `par_iter` closures — causes contention and serializes execution
- Avoid blocking I/O inside `par_iter` — use Tokio for I/O, Rayon for computation
- `par_iter` closures must be `Send` — no `Rc`, no non-Send types

## Bridging with Tokio

```rust
// Run CPU-bound work from async context
let (tx, rx) = tokio::sync::oneshot::channel();
rayon::spawn(move || {
    let result = expensive_computation();
    let _ = tx.send(result);
});
let result = rx.await.unwrap();
```

## Reduction Patterns

- `par_iter().sum()`, `.min()`, `.max()` for simple reductions
- `par_iter().reduce(identity, combine_fn)` for custom reductions
- `par_iter().fold(init, fold_fn).reduce(identity, combine_fn)` for fold-then-reduce (when fold state is expensive to create)

## Scoped Parallelism

- `rayon::scope(|s| { s.spawn(|_| work()); })` — spawn tasks that can borrow from the enclosing stack frame
- Scoped tasks are guaranteed to finish before `scope()` returns — safe to pass `&T` references without `'static`
- Use `rayon::join(|| a(), || b())` for simple two-task fork-join without needing a scope

## Advanced Iteration

- `par_iter().map_with(init, |state, item| ...)` — thread-local mutable state per worker (avoids locking)
- `par_iter().flat_map_iter(|item| sequential_iter)` — flatten with a sequential inner iterator per item
- `par_iter().panic_fuse()` — stops all threads as soon as a panic occurs (default propagates but doesn't halt immediately)
- `par_iter().while_some()` — halts as soon as any element produces `None`
- Collect directly into `HashMap`: `par_iter().map(|x| (k, v)).collect::<HashMap<_,_>>()`

## Panic Handling

- Panics inside `par_iter` closures are propagated to the calling thread at `.collect()`/`.for_each()` completion
- Without `panic_fuse()`, other threads continue executing after a panic — add it when early abort is critical
- `std::panic::catch_unwind` inside a `par_iter` closure to handle per-item panics gracefully
