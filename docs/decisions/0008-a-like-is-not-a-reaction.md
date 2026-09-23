# ADR 0008 — A like is not a reaction

**Date:** 2026-09-23
**Status:** accepted
**Decision:** treff gets **one** like per person and post — a heart, a number,
an entry in the author's bell and no mail. Emoji reactions stay out, and
`design.md` §5 now says so in those words instead of "reactions".

## Context

`design.md` §5 listed reactions under "left out permanently", with the rule
that whoever misses one changes that section before writing code. The
operator asked for likes on 2026-09-23, on both spaces: every post shows how
many, a button to add yours, and the author hears about it — in the bell,
not by mail.

The original reason for leaving reactions out was size, and it still holds
for what most forums mean by the word: a palette of emoji, a count per emoji,
a list of who chose which, and a settings page for the palette. That is a
feature with a surface of its own, and on a closed circle of people who know
each other by name it answers a question nobody asked.

A like is smaller than that, and it answers a real one. In a circle this
size most posts get no reply, because there is nothing to add — and then the
writer cannot tell whether anybody read it. "I read this and I am glad it is
here" is worth one click and one number. It is the one reaction that does
not turn a post into a poll about itself.

## What is decided

1. **One like, not a palette.** A `likes` row is a post and a person; there
   is no `kind`. Adding a second kind later would be a new decision, and this
   ADR is where it would be argued.
2. **Told, not mailed.** A like writes an entry in the author's bell and
   nothing in the outbox. The mail exists for things somebody may need to
   act on; a like is not one, and a mail per like would teach people to
   ignore the mails that matter. There is no switch for this, because a
   switch would be a second code path to keep true.
3. **Bundled per post, re-opened by the next like.** One entry per post, its
   number and latest name read from `likes` when the bell is shown. A new
   like on a post whose entry was read makes that entry unread again; the
   last like taken back removes it. The bell never says something the count
   under the post does not.
4. **Your own post has no button**, and the route refuses it. Liking
   yourself would be counted, and the count is the whole point. "Your own"
   is the name on the post: the owner of a space's mirrored articles may
   like them (0.10.1) — the article is stored under a name nobody signs in
   as, and the owner's click only rings no bell of their own.
5. **Reading is the right that is needed.** `may_read` of the space, not
   `may_reply` of the category: somebody who may read an article but not
   comment may still say they liked it.

## Consequences

`design.md` §5 changes with this ADR. `inbox.reason` grows a third value;
the migration rebuilds the table because SQLite cannot widen a `CHECK`. A
third script, `like.js`, joins the two of ADR 0005 on the same terms: from
`'self'`, no inline code, nothing depends on it — without it the like is a
form and the page reloads at the post.
