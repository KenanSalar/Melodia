# ADR 30: Memory is a product requirement, and three knobs hold it

**Status:** Accepted, 2026-05-25

The previous version's footprint is why this one exists (ADR 2), so "it is only idle memory" is
not an argument that works here. Two of the biggest movers turned out not to be anything the
application allocated, which is exactly why they were hard to find.

Decision: memory is treated as a requirement with a published number rather than as something to
look at when it gets bad. Peak resident size is measured after notable changes, a feature adding
appreciably to idle memory gets profiled before it merges, caches are bounded, and three specific
knobs are set deliberately.

Alternatives: leaving allocator behaviour at its defaults and treating the result as the platform's
business; swapping the global allocator for one of the well-known alternatives; treating memory as
a thing to look at only when a user complains.

Trade: the first knob is the allocator's arena count. The drift that looked like a cache leak in
the UI toolkit was per-thread arenas: the default scales with core count and each one reserves a
large virtual region, so a machine with many cores and a process with many threads grows without
anything leaking. Capping it has to happen before the first allocation on any thread, which is
before the logger and before the runtime, both of which allocate, so it is the first thing `main`
does. It must not be capped to one, because that serialises every allocation and the audio thread
can then stall behind the UI.

The second is a one-shot trim shortly after startup, which returns what the scan and the boot path
grew into and never needed again. It sits with the cap in one module, because the two are one
concern and separating them is what let a threshold drift out from under the other.

The third is the blocking pool ceiling. The runtime's default allows several hundred threads, and
the risk there is not steady state but a burst: nothing bounds how many blocking tasks are in
flight, and the tenants that never return a slot are the ones that make the ceiling reachable. The
idiom that makes a low ceiling comfortable is one blocking task wrapping a parallel iterator,
never one per item.

Swapping the global allocator was tried and reverted. The obvious candidate's per-thread segments,
multiplied across the thread count this process actually runs, cost substantially more than they
saved.

What all this costs is that a plain library upgrade can move the number, and someone has to notice.
The measurement is manual and nothing in CI enforces it, so this is a discipline rather than a
gate, which is the weakest form of any of these decisions.

This ADR was written in September 2026. The arena cap predates the repository's first commit; the
blocking pool ceiling was set on 2026-08-14.
