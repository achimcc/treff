# ADR 0003 — Times are shown with the hour, in a zone the instance names

**Date:** 2026-09-11
**Status:** accepted (reverses a decision that lived in a doc comment)
**Decision:** every date treff shows carries the hour and the minute, rendered
in one zone fixed at startup — the machine's own, or the IANA name in
`timezone`. An unresolvable name stops the program.

## Context

Until now a timestamp reached a page through `views::day`, which rendered the
Unix seconds as a bare `YYYY-MM-DD` in UTC. Its doc comment argued the case:

> No clock: in a forum for a closed circle the hour is noise, and a date needs
> no time-zone argument.

The second half is what the first half bought. `OffsetDateTime::from_unix_timestamp`
is UTC, and a bare date hides that: in Berlin it is wrong for two hours out of
every twenty-four in winter and one in summer, which nobody notices because
nothing is shown that could disagree.

## Why that was reversed

1. **The hour is not noise, it is the order.** A topic list sorts by last
   activity. With a bare date, a thread answered this morning and one answered
   a week ago last Tuesday look equally recent, and two posts written in the
   same afternoon lose the sequence they were written in — which is the one
   day the sequence is worth reading.
2. **The forum grew a second column.** A row now says who opened a thread and
   who answered last. Two dates in a row with no hour on them invite exactly
   the wrong reading: that nothing has happened since.
3. **The time-zone question was not avoided, only hidden.** Shown, it has to
   be answered, and answering it costs one configuration key.

## The zone, and why not the alternatives

**A fixed offset in the configuration** (`+02:00`) needs no library and is
wrong for half the year: somebody has to correct it by hand twice a year, and
this project treats a manual step as a fault.

**The reader's zone** would need JavaScript, which this project does not have,
or a per-reader setting nobody asked for. A forum for a closed circle is read
in the circle's own time.

**UTC, labelled as such**, is honest and makes every reader do arithmetic.

So: an IANA name, resolved through `jiff`, which reads the system zone
database and knows when the rules change. `time` — already in the tree for the
cookies — cannot do this: `UtcOffset::current_local_offset` refuses to answer
in a multi-threaded process, for reasons of soundness rather than policy, and
`time` carries no zone database at all.

## Consequences

- One dependency more: `jiff`. Pure Rust, no build script, reads
  `/etc/localtime` and `/etc/zoneinfo`, both of which survive the unit's
  `ProtectSystem = "strict"`.
- `timezone` in the configuration file, and `services.treff.timezone` in the
  NixOS module. Both default to the machine's zone, so an instance that stands
  where its readers do configures nothing.
- A name with a typo in it stops the program at startup, like every other
  unusable value in that file.
- The zone is fixed for the life of the process. Two pages cannot disagree
  about what time it is, and an instance moved to another zone is restarted —
  which it would be anyway.
