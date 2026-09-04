# ADR 26: The backdrop is bounded washes of the cover's colours over the theme's own base

**Status:** Accepted, 2026-08-17

The first backdrop solved the surface and the foreground separately: it painted a blurred cover
behind the Now Playing view and then adapted the text colour to whatever that cover turned out to
be. Both halves were solved per cover, so both had to be right for every cover, and the failure
mode was a light album against light ink on a surface neither half owned.

Decision: the backdrop is washes of the album's own colours laid over the theme's base colour,
with the composite capped so it cannot leave the band the theme's own ink was chosen against. The
foreground stops adapting to the cover entirely.

Alternatives: keeping the shipped model and fixing its contrast case by case; a fixed opacity cap
taken from elsewhere; deriving the palette with the same quantizer the dynamic theming already
uses.

Trade: the point is that legibility becomes a property of the construction rather than of a
measurement. If the composite provably stays inside the band the theme's text was picked for, then
the text does not have to be picked again per cover, and a whole chain of measuring the surface and
solving for a scrim disappears. One clamp replaces it. That is the difference between a rule that
holds for every cover including the ones nobody tested and a rule that holds for the covers
somebody looked at.

The cap is computed per theme rather than fixed, because six palettes ship and they do not sit at
the same lightness, so one number would be too dark for some and too weak for others. It is a
closed form over the theme's own base and ink, which are already known, so it costs a computation
rather than a table somebody has to maintain.

The quantizer is the second one in the tree rather than a reuse of the first, and that is
deliberate. The palette-derivation quantizer answers a usability question about which colour should
be an accent, which is not what a backdrop is asking, and it is orders of magnitude slower on the
same input. A backdrop wants the dominant colours quickly. Carrying two is the cost.

What this trades away is fidelity: the backdrop no longer looks like the cover, it looks like the
cover's colours, and someone who liked seeing the artwork behind the view has lost that.

**Amendment, 2026-09-02:** this shipped alongside the older blurred-cover backdrop with blur as the
default, on the grounds that it was the known quantity. The default is now the aurora. Saved
installs keep whatever they had, and the README screenshot still shows the blur.

This ADR was written in September 2026 from the aurora working doc, deleted when the feature
shipped. That doc had already rewritten itself once to reverse its own opening premise, which is
why the alternative listed first is its own earlier design.
