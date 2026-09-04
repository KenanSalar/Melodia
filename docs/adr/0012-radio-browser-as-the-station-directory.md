# ADR 12: The station directory is radio-browser.info

**Status:** Accepted, 2026-08-20

Internet radio only means something if a user can find a station. Melodia has no server, no
account system and no editorial staff, so where the station list comes from decides whether the
feature is useful or a URL box with a nice frame around it.

Decision: stations are searched through the radio-browser.info directory, over its documented
mirror list, with no API key and no account.

Alternatives: a station list bundled with the app, Icecast's own directory, and URL entry only.

Trade: a bundled list is the option that needs no network at all and it is a maintenance tax paid
forever. Stations move, rename and go off air, so the list rots between releases and the only fix
is a release, which makes the feature worse every day since it shipped and still cannot satisfy
"any radio in the world". Icecast's directory is real and free but only lists stations that opted
into its yellow pages, which is a small and hobby-skewed fraction of what exists. URL entry alone
is the cheapest thing to build and it is useless to anyone who does not already know a URL, which
is nearly everyone.

radio-browser has over sixty thousand stations, needs no key and no account, publishes its data as
CC0 and publishes a mirror list rather than a single host. What it costs is a hard dependency on a
volunteer-run service for the discovery half of a feature: if it is down, search is down, and if it
disappears, this decision has to be made again. That is survivable in a way the alternatives are
not, because the half a user actually keeps, their favourites and their own URLs, is in the local
database (ADR 13) and does not go anywhere. Discovery degrades; the library does not.

Its data is also user-contributed, which means the directory returns entries that are wrong,
duplicated, or that nobody should be handed. That is the reason the directory sits behind a single
facade with a filter in it rather than being called from wherever a list is needed.
