# ADR 1: Record architecture decisions here, and argue everything else at its anchor

**Status:** Accepted, 2026-09-04

A working doc gets deleted when its feature ships. That has happened to fourteen of them, and to
two `.claude/rules/` entries beside them, and each time the reasoning went out with the file.
`CROSSFADE.md` was dropped in `9bf2794` while crossfade kept running, so the argument for two
decks and a complementary linear ramp now survives only in the body of the commit that deleted
it, which nobody is going to go looking for. The history has a floor under that: the root commit
is the migration from the Tauri application and its body is empty, so the largest decision in the
project is not recoverable from the repository at all. Meanwhile "why this and not the obvious
alternative" keeps being asked and keeps being answered from scratch, because no tier answers it.
The doc comments and the rules say how a thing works, `CLAUDE.md` says what you may not do, and
the choice itself has nowhere to live. There is no line of code that owns "sqlx rather than
rusqlite".

Decision: a choice that spans the whole project and has no file to sit beside gets a numbered ADR
in `docs/adr/`, one per file, written for a person. Everything else stays where it already is. A
tuning constant is argued in the doc comment on the constant, where it cannot drift out of sight
of the code it describes. A contract spanning trees that no single file reaches is a
`.claude/rules/` entry. A prohibition violable from anywhere is a bullet in `CLAUDE.md`. A
feature's implementation shape is a working doc under `docs/plans/`, still as temporary as it is
today, except that one reaching the point of deletion now leaves an ADR behind first. That last
clause is the whole mechanism, and it is attached to something that already happens rather than
to a habit somebody has to remember.

Alternatives: keeping the working docs instead of deleting them; a single `DECISIONS.md`; MADR;
leaving the arguments in commit messages, which is the status quo.

Trade: this is a fourth place to look, and a tier that can go stale like any other. What makes it
worth the fourth place is that nothing else here can hold a reversal. An ADR is dated and
superseded rather than edited, so a stale one is legible as stale, where a paragraph in the README
that quietly stopped being true is not, and a deleted working doc is not even that. The other
three lose to the same test. Keeping the working docs means keeping documents that go stale as
plans while still reading as authoritative, which is why the convention deletes them. A single
file is rewritten in place and gives up the one property being paid for. MADR was built for teams
of fifteen to fifty and for regulated contexts: its front matter records who was consulted and who
was informed, which is nobody here, and its pros-and-cons-per-option tables run to two to four
pages and are the exact part people stop reading. The compression in `README.md` is what keeps
this from turning into that.

What it costs on the way in is a backfill, and the low numbers here are retroactive: written from
the code, from working docs recovered out of git, and from commit messages, months after the fact.
Each of those says so in its own text. The backfill is also bounded and listed rather than
open-ended, because the way a collection like this actually dies is five files in the first month
and then nothing for a year.
