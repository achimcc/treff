# ADR 0005 — One script, for mentions, and nothing depends on it

**Date:** 2026-09-22
**Status:** accepted
**Decision:** treff ships its first script: `/assets/mention.js`, served from
its own route like the stylesheet, no library, no inline code. The CSP moves
from `script-src 'none'` to `script-src 'self'` and no further. Every page
works exactly as before without it.

## Context

0.5.0 made `@handle` a mention. Without script, a handle has to be known: it
stands next to the author's name on every post, and people copy it from
there. The operator asked for more on the day 0.5.0 shipped: *when somebody
types `@`, show the list of everybody as an overlay to choose from; when they
go on typing after the `@`, complete it.*

No HTML element does that. `<datalist>` completes a single-line `<input>` from
its first character, not a word in the middle of a `<textarea>` after an `@`.
There is no way to open a list at the caret without script.

Until now every page said so in its source: "no script, no third party", and
the CSP enforced it with `script-src 'none'`. That was not decoration — it is
the reason a stored XSS in a post body would find nothing to run in.

## What changes, and what does not

1. **`script-src 'self'`, not `'unsafe-inline'`, not a hash, not a CDN.** A
   post body still cannot run anything: comrak omits raw HTML, ammonia strips
   `<script>` and every `on…` attribute, and the CSP forbids inline script
   regardless. What `'self'` adds is exactly one thing a page can load — a
   file treff itself serves.
2. **The script is an enhancement, never a dependency.** No form, link or
   button needs it. Without it (blocked, failed, disabled) `@handle` is typed
   by hand as in 0.5.0. A test proves the pages carry no `onclick` and no
   inline `<script>`.
3. **The list it shows is decided by the same rule as the mention.** Only
   accounts whose groups let them read the space — `mentions::readers`, the
   function that already decides who is told and what is highlighted. A
   suggestion list that showed everybody with an account would say exactly
   what a mention is careful not to say: who exists.
4. **Loaded once per page, filtered in the browser.** The overlay appears the
   moment `@` is typed; a request per keystroke would make it lag, and the
   whole list of a closed circle is small. The endpoint returns name and
   handle and nothing else — no address, no subject.

## Consequences

The sentence "there is no script on any page" is no longer true, and the
comments that say it (views, stylesheet) are changed with this ADR rather
than left to mislead. Reviewing a change to `mention.js` is now part of
reviewing treff; it is small on purpose so that stays possible.
