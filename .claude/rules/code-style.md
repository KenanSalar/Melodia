---
paths:
  - src/**/*.rs
  - src/error.rs
  - src/main.rs
  - src/lib.rs
  - melodia-ui/src/**/*.rs
  - tests/**/*.rs
  - build.rs
  - melodia-ui/build.rs
  - melodia-ui/ui/**/*.slint
  - migrations/**/*.sql
  - scripts/*.py
  - scripts/*.sh
---

# Code style and comments

## Writing Code Like a Senior Engineer

Write every change as an experienced human engineer would — code a senior colleague
would approve without rewriting, indistinguishable from thoughtful hand-crafted work.
Optimize for the next person who reads it (usually your future self), not for looking
clever or finishing fast.

- **Clarity over cleverness** - clear beats clever every time. Expand a clever one-liner
  into a few obvious lines when it reads better. Readable code scales; clever code ages badly.
- **Write for the reader** - code is read far more than written. Make intent obvious on the
  page; don't make the reader hold state in their head or decode `u`, `s`, `tmp`.
- **Name things precisely** - intention-revealing names (`calculateTotalPrice`,
  `isSubscriptionActive`), never `d`, `x`, `foo`, or a bare `data`. A good name removes the
  need for a comment.
- **Small, single-purpose functions** - one job, one level of abstraction. If you're
  narrating "step 1… step 2…", split it into functions a reader can hold in their head.
- **Guard clauses over nesting** - return early and flatten arrow-shaped `if/else` pyramids;
  handle the edge/error case first, then the happy path.
- **No magic values** - lift magic numbers/strings into named constants or config; the name
  documents the intent.
- **Minimize side effects** - prefer functions that take input and return output; centralize
  state mutation and I/O rather than scattering it.
- **Reduce, don't just add** - seniors delete. Remove dead code, collapse duplication, prefer
  the smaller solution, and leave each file a little cleaner than you found it (boy-scout rule).
- **Match the surrounding code** - mirror the existing naming, structure, error-handling, and
  style of the file. Consistency beats personal preference.
- **Handle errors and edges deliberately** - no silent catches or ignored failures; make
  failure modes explicit and intentional.
- **Be pragmatic, not dogmatic** - these are defaults, not laws; knowing when to bend one for
  clarity or simplicity is what separates senior from junior. (Complements **Principles** and
  **Antipatterns to Avoid** in the global `CLAUDE.md` — don't duplicate them.)

Avoid the tells of machine-generated code: comments that restate the code, defensive
boilerplate nobody asked for, needless abstraction, over-verbose names, and style that drifts
within a file. The result should look like a thoughtful human wrote it.

## Comments

Comments are a liability, not an asset — each one is prose you must keep true. Write few, make
each earn its place, and re-read every comment before moving on.

- **Explain *why*, not *what*** - the code already shows what it does; a comment justifies a
  non-obvious choice, trade-off, or gotcha. If you're narrating the code, delete it.
- **Prefer self-documenting code** - clear names and small functions beat comments. If a comment
  has to explain "step 1… step 2…", the function is too big — split it instead.
- **No redundant comments** - never restate a name, type, or the obvious (`// increment i` over
  `i++`; `// the user's name` over `userName`).
- **Be terse** - one line where one line works; no decorative banners, ASCII art, or boilerplate.
- **Sound like a senior engineer, not a generator** - write comments in plain, natural prose,
  the way a human colleague jots a note: terse, direct, peer-to-peer. Avoid the machine tells —
  restating the signature, "This function…", robotic step-by-step narration, hedging, and
  decorative filler. Assume the reader knows the language.
- **Double-check every comment** - confirm it is accurate and *still* true. When you touch code,
  update or delete the comments around it; a stale or wrong comment is worse than none.
- **Comment the genuinely non-obvious** - empty catch blocks, unsafe casts, workarounds (link the
  issue/PR), non-trivial regexes, magic constants, and concurrency/ordering assumptions.
- **No commented-out code** - delete it; version control remembers.
- **Match the file** - follow the surrounding code's existing comment *style*. Not its density:
  a dense neighbour is not a budget, and the rules above outrank it.
- **Document public APIs** using the doc-comment format below. Start with a single-sentence
  summary; don't just repeat the member name.

**Doc comments in this tree:**

| Language | Doc syntax | Notes |
|----------|-----------|-------|
| Rust | `///` outer, `//!` module/crate | Markdown; 3rd-person summary ("Returns…"); `# Examples`/`# Panics`/`# Errors`. Doc examples don't run here — `[lib] doctest = false` |
| Slint | none — `//` and `/* */` only | Nothing extracts docs from `.slint`, so a note above a component, property or callback is the whole affordance; keep it to why the binding is shaped that way |

SQL migrations (`--`) and the `scripts/` helpers (`#`) have no doc-comment form either; the
generic rules above are the whole contract there.
