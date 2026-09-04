# ADR 15: Radio ships off, and the guard is at the facade rather than the UI

**Status:** Accepted, 2026-08-20

Melodia's package description, already shipped to every distribution, says the collection stays on
your own machine with no accounts, streaming or cloud. Radio makes that false for anyone who turns
it on, and it must not make it false for anyone who does not.

Decision: the feature ships disabled. One toggle removes it, and the guard that actually stops
traffic is a single early return in the radio facade, which is the one door every directory call and
every logo download goes through. The UI gates are cosmetic on top of that.

Alternatives: shipping it on, since it is the feature that was just built; gating it in the UI by
not mounting the section; a per-surface gate on each thing that makes a request.

Trade: the gate that matters is not the one on the sidebar. What a user turning this off is buying
is "no traffic", and a UI gate does not sell that: it hides the entrance while every fetch behind it
is still reachable from anything that calls it. Putting the return at the facade means there is one
place to enforce it and a grep can prove there is only one, which is the whole reason the facade
exists as a module rather than as a convention. The UI gates are still needed, because nothing
should mount and start fetching, but they are the second line rather than the first.

Default off costs the feature its audience on upgrade: it exists and nobody sees it until they go
looking, which for a feature this size is a real loss and the strongest argument for the other
default. It loses to the shipped description. A doc comment resting on a blurb the feature falsifies
is worse than either alone, and the honest options were to change the default or change the
description, not to leave both.

What the toggle costs is that disabling is not just an unmount. A station currently playing has to
stop, the navigation history has to forget the section so no back-navigation lands on it, the
selection has to move if that is where the user is standing, and a persisted index pointing at it
has to be folded on read at the next boot. Four consequences, and they are the real work of the
switch.
