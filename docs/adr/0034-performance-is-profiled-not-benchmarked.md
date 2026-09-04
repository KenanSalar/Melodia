# ADR 34: Performance is held by profiling and by shape, not by a benchmark suite

**Status:** Accepted, 2026-09-04

Two paths in this app have to be fast or the product is broken, and they break differently. The
audio callback pulls the whole DSP chain: too slow, or one allocation, or one lock, and the output
block misses its deadline and the user hears it. The UI tick redraws a visualizer and animations
against a display that may be running at 144 Hz: too slow and frames drop while the app is doing the
one thing it exists to do. Neither failure is something a unit test notices, and the README
publishes CPU figures, so the claim is public rather than internal.

Decision: performance is measured at the point of change with a profiler on the real workload, using
flamegraph, heaptrack and peak resident size, and the two hot paths are held by structural rules
rather than by timings. There is no benchmark suite, no criterion, and no CI performance gate.

Alternatives: criterion benchmarks with a regression gate on the pull-request path; a periodic
performance job; measuring nothing and reacting to reports.

Trade: a timing gate on a shared CI runner is noisy, and a gate that fails intermittently is worse
than no gate at all, because the first few false alarms teach everyone to ignore it and then it is
still red when something real lands. The deeper problem is that microbenchmarks would have missed
every regression actually found here. The dominant per-frame cost in the visualizer turned out to be
number formatting rather than arithmetic, asking for an exactly-rounded decimal where an integer
print would do. The backdrop's cost was a transcendental called three times per pixel over a domain
of 256 values, fixed with a lookup table. The memory drift was per-thread allocator arenas
([ADR 30](0030-memory-is-a-product-requirement.md)). None of those is a function somebody would
have thought to benchmark, and all three were obvious in a profile of the real thing.

The structural half is what actually keeps the audio path safe, and it is checkable where a timing
is not. The callback allocates nothing, takes no lock and touches no socket
([ADR 14](0014-no-network-on-the-audio-callback.md)), parameter changes reach it through
lock-free cells polled by a counter ([ADR 10](0010-one-dsp-wrapper-lock-free-state.md)), and
the per-sample path is iterator-based with no index in it. Those are properties of the code's shape,
so they are reviewable and testable in a way that "is it under a millisecond on this machine" is
not.

The cost is real and it is the same weakness [ADR 30](0030-memory-is-a-product-requirement.md)
names: nothing catches a gradual regression. A change that makes something twenty percent slower
ships unless somebody profiles, and whether somebody profiles depends on remembering to. This is a
discipline rather than a gate, and it holds only while the person making the change is the person
who cares about the number.

The rule that follows from all of it, and the one worth keeping if the rest is ever revisited: the
win comes from asking a smaller question, not from removing a check. Every hot spot here was fixed
in safe code that did less work, never by reaching for `unsafe`
([ADR 28](0028-no-unsafe-outside-ffi-no-unwrap-anywhere.md)).

This ADR was written in September 2026 from `.claude/rules/rust-performance.md`,
`.claude/rules/unsafe-rust.md` and `.claude/rules/testing.md`.
