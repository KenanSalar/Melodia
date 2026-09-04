# ADR 22: Smart playlist membership is derived at read time

**Status:** Accepted, 2026-07-08

A smart playlist is a set of rules over the library rather than a list somebody assembled, and the
library moves under it constantly: a scan adds tracks, a rating changes, a play count goes up. So
the question is whether membership is a thing that is stored and maintained, or a thing that is
computed when someone looks.

Decision: membership is a query. The rules are stored, the tracks are not, and opening a smart
playlist builds SQL from the rules and runs it. Nothing writes playlist rows for one.

Alternatives: materializing membership into the ordinary playlist items table and keeping it up to
date; a cached membership refreshed on a timer or after a scan.

Trade: materializing means every write path that could change a track has to know which smart
playlists it might have moved it into or out of, which is every rating change, every play, every
scan and every tag edit. That is a fan-out with no natural boundary, and every one of those paths
would be wrong in the same silent way: the playlist is stale and looks fine. Deriving means the
rules are the only state, so there is nothing to be stale.

Opening one is then a query rather than a lookup, and it cannot be paged as cheaply as a fixed list.
It also means a smart playlist has no drop target, because dropping a track on it would mean
nothing, and that asymmetry with ordinary playlists is visible in the UI.

The query is built rather than written, which is where the risk moved. Only enum-derived static
fragments reach the SQL as text: the column name, the operator token and the ordering. Every user
value goes through a bind. Relative dates are resolved to a cutoff in Rust before they get near the
query. That contract is the reason a rule set can be user-authored at all, and it is the thing to
preserve if the rule vocabulary ever grows.

This ADR was written in September 2026 from the smart playlists working doc, deleted when the
feature shipped.
