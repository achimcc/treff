# Stage 7 — likes, and the face of the forum

The design is `design-likes-and-the-forum-face.md`; the decision that opens
the door is ADR 0008. Same cycle as every stage (`CLAUDE.md`): the test first,
red, the smallest thing that passes, green with clippy and fmt, commit by
name. Before the tag, `nix flake check` and the preview.

**In one sentence:** one heart per person and post, counted under the post,
told to the author in the bell and never by mail — and the forum's pages
answer where something is going on, how long a thread is and where you are.

**Global constraints (from the design and `CLAUDE.md`):** no inline script,
no inline style, CSP stays `script-src 'self'`; nothing depends on `like.js`;
every user-facing string in `i18n/*.toml`, both languages, ASCII in German as
the file has it (`erwaehnt`, not `erwähnt`); `sqlx::query`, never
`query!`; no `unwrap()`/`expect()` in a request path; a post from the other
space is 404; the writer of a like sees nothing in their own bell; a like
writes no outbox row.

**Review focus — what the tests must pin so a person is not bitten:**
1. A like on a post whose topic is `hidden` (a withdrawn article): counted
   under the post, not in the bell (`unread_count` joins `topics.hidden = 0`
   already; the likes query must too).
2. Two likes arriving in the same second on one post: one entry, count 2 —
   the upsert must not race into two rows (primary key holds it).
3. `like.js` on a failed fetch (offline, 5xx): the form is submitted the
   ordinary way, the page reloads, nothing is lost.
4. A topic in a category the configuration no longer has, on the front
   page's recent list: shown with its slug as label, its link still works.
5. A reader who may read but not reply: the heart is a button for them.

## Status

| | |
|---|---|
| Done | **Task 1** — `likes`: table, toggle, counts, the entry in the bell |
| Done | **Task 2** — `POST /p/{id}/like`, the heart on the pages, `like.js` |
| Done | **Task 3** — the bell says it: page, JSON, `bell.js` |
| Done | **Task 4** — the face of the forum: recent, sections, counts, numbers |
| Done | **Task 5** — preview by eye, 0.9.0 |

---

## Task 1 · `likes`: table, toggle, counts, the entry

**Files:** `migrations/0014_likes.sql` (new), `src/db/likes.rs` (new),
`src/db/mod.rs` (the module), `src/db/inbox.rs`

- [x] Migration:

  ```sql
  CREATE TABLE likes (
      post_id    INTEGER NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
      subject    TEXT    NOT NULL,
      name       TEXT    NOT NULL,
      created_at INTEGER NOT NULL,
      PRIMARY KEY (post_id, subject)
  );
  -- inbox.reason gains 'like'. SQLite cannot widen a CHECK: rebuild.
  CREATE TABLE inbox_new ( … same columns …, reason CHECK (reason IN ('reply','mention','like')) …);
  INSERT INTO inbox_new SELECT * FROM inbox;
  DROP TABLE inbox; ALTER TABLE inbox_new RENAME TO inbox;
  CREATE INDEX inbox_unread ON inbox(subject, space, read_at);
  ```

- [x] `db::likes`:
  - `pub enum Outcome { Toggled { liked: bool, count: i64, topic_id: i64 }, OwnPost, NotFound }`
  - `pub async fn toggle(db, space: &str, post_id: i64, who: &Identity) -> Result<Outcome>`.
    One transaction: the post joined to its topic by `space` (else
    `NotFound`); author == `who.subject` → `OwnPost`; if a row exists,
    delete it, and if the count is now 0 delete the inbox row
    `(author, post_id, reason='like')`; else insert the like and upsert the
    entry:
    ```sql
    INSERT INTO inbox (subject, space, topic_id, post_id, reason, created_at)
    VALUES (?, ?, ?, ?, 'like', ?)
    ON CONFLICT (subject, post_id) DO UPDATE
       SET read_at = NULL, created_at = excluded.created_at
     WHERE reason = 'like'
    ```
    Commit, then `db.changed(space)`. Returns the new `count`.
  - `pub struct Summary { pub count: i64, pub liked: bool, pub names: Vec<String> }`
    (names: the latest five, newest first — for the heart's `title`).
  - `pub async fn summaries(db, post_ids: &[i64], viewer: &str) -> Result<HashMap<i64, Summary>>`,
    one query over `likes WHERE post_id IN (…)`, folded in Rust.
- [x] `db::inbox`: `Entry::Likes { topic_id, topic_title, post_id, count, latest_name, at, unread }`;
  `unread()`/`at()` arms; `unread_count` adds
  `count(CASE WHEN i.reason = 'like' THEN 1 END)` inside the same join
  (so `hidden = 0` holds); `entries()` adds a third query:
  ```sql
  SELECT i.topic_id, i.post_id, i.created_at AS at, (i.read_at IS NULL) AS unread,
         t.title, (SELECT count(*) FROM likes l WHERE l.post_id = i.post_id) AS n,
         (SELECT name FROM likes l WHERE l.post_id = i.post_id ORDER BY created_at DESC, rowid DESC LIMIT 1) AS latest
    FROM inbox i JOIN topics t ON t.id = i.topic_id
   WHERE i.subject = ? AND i.space = ? AND i.reason = 'like' AND t.hidden = 0
  ```
  An entry whose `n` is 0 (cannot happen after `toggle`, can after a
  restore) is left out rather than shown as "nobody likes".
- [x] Unit tests (`src/db/likes.rs`, `src/db/inbox.rs`): toggle on, toggle
  off, own post, other space; the author's `unread_count` is 1 after one
  like and still 1 after a second liker; the entry carries count 2 and the
  latest name; `mark_topic_read` reads it; the next like re-opens it; the
  last unlike removes it; a deleted post leaves no like and no entry; the
  liker's own bell is 0; **`outbox` has no row** (`SELECT count(*) FROM outbox`).
- [x] `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check`. Commit: `Stage 7, task 1: likes in the database and
  in the bell`.

## Task 2 · The route, the heart, `like.js`

**Files:** `src/web/mod.rs`, `src/web/views.rs`, `src/web/like.js` (new),
`src/web/style.css`, `i18n/en.toml`, `i18n/de.toml`, `tests/likes.rs` (new),
`tests/frame.rs` (the script count), `tests/js/like.html` (new),
`tests/js/run.sh`, `tests/js/expected.txt`

- [x] `POST /p/{id}/like` → `like_post`: `may_read` or 403; `toggle`;
  `NotFound` → 404, `OwnPost` → 403; `Accept` containing
  `application/json` → `200 {"liked": …, "count": …}` with
  `Cache-Control: no-store`; else `303` to `/t/{topic}#p{id}` (the topic id
  comes back from `toggle` in `Outcome::Toggled` — add `topic_id` there).
- [x] `GET /assets/like.js` like the other two (`script()` helper, its own
  ETag); the layout loads it with `defer`; `tests/frame.rs` counts three
  `<script` now.
- [x] Views: `TopicView` gets `likes: &HashMap<i64, likes::Summary>`;
  `TopicRow` gets `likes: Option<likes::Summary>` for the timeline's opening
  post. A helper `like_control(lang, post_id, own: bool, summary)`:
  ```html
  <form class="like" method="post" action="/p/7/like">
    <button type="submit" aria-pressed="false" title="ada, ben"
            aria-label="Like">  <svg class="icon heart">…</svg> <span class="count">2</span></button>
  </form>
  ```
  On your own post: `<span class="like own" title="ada, ben">♥ 2</span>`
  (same SVG, no button), and nothing at all when the count is 0. The count
  span is omitted at 0 on the button too — a heart alone reads as "none".
- [x] i18n: `like = "Like"/"Gefaellt mir"`, `unlike = "Unlike"/"Gefaellt mir nicht mehr"`,
  `likes_one = "1 like"/"1 Gefaellt mir"`, `likes_many = "likes"/"Gefaellt mir"`
  (for `title` and screen readers — `aria-label` says like/unlike by state).
- [x] `like.js`: for every `form.like`, on `submit` → `preventDefault`,
  `fetch(action, {method:"POST", credentials:"same-origin", headers:{Accept:"application/json"}})`;
  on `ok` set `aria-pressed`, `aria-label`, and the count span (create at
  >0, remove at 0); on anything else `form.submit()` — the ordinary way,
  never a silent failure. No `innerHTML`.
- [x] `style.css`: `.like` (inline-flex, at the post's foot, mono 12px,
  `--murmur`); `.like button` borderless; `[aria-pressed="true"] .heart`
  filled (`fill: currentColor`, colour `--on`); hover `--on`; focus ring as
  everywhere; `.like.own` static. Place it in the post's foot line together
  with `.own` (edit/delete) — one flex row, heart left, own controls right.
- [x] Route tests (`tests/likes.rs`): a like shows `class="count">1<`, the
  liker's page has `aria-pressed="true"`, a second POST removes it; JSON
  answer; own post 403 and no `form class="like"` on it; the other space
  404; a reader with only `read` (no `reply`) sees the button; after
  deleting the post the like is gone.
- [x] `tests/js/like.html`: a page with two forms and a stubbed `fetch`
  (first answers `{liked:true,count:3}`, second rejects); record
  `aria-pressed`, the count text, and that the rejected one called
  `form.submit` (stub it). `run.sh` takes a fourth page; `expected.txt`
  gains a line. Run it: `bash tests/js/run.sh`.
- [x] Green ×4, commit: `Stage 7, task 2: the heart under every post`.

## Task 3 · The bell says it

**Files:** `src/live.rs`, `src/web/bell.js`, `src/web/views.rs`
(`notifications_page`), `i18n/*.toml`, `tests/js/bell.html`,
`tests/js/expected.txt`, `tests/inbox.rs`

- [x] `entry_json`: `Entry::Likes` → `{"kind":"likes","title","count","author": latest_name,"at","unread","link": "/t/{topic}#p{post}"}`.
- [x] i18n: `likes_your_post_in = "likes your post in"/"gefaellt dein Beitrag in"`,
  `and = "and"/"und"`, `others_like_your_post_in = "others like your post in"/"weiteren gefaellt dein Beitrag in"`;
  wording: count 1 → `{author} {likes_your_post_in} {title}`; count n →
  `{author} {and} {n-1} {others_like_your_post_in} {title}`.
- [x] `notifications_page`: the arm, link to the post, byline the moment.
  `nothing_new` mentions likes now (both languages).
- [x] `bell.js`: the `likes` branch with the same wording from three new
  `data-t-*` (`likes-your-post-in`, `and`, `others-like-your-post-in`); the
  bell element in `layout` carries them. `tests/js/bell.html`: a `likes`
  entry with count 3 in the pushed frame; `expected.txt` renewed with
  `PRINT=1` and read before committing.
- [x] Route tests (`tests/inbox.rs`): a like from ben puts `1` on ada's
  badge and a line "ben the tester likes your post in *T*" on the page;
  cem's like turns it into "cem the tester and 1 others like your post in"
  (the latest name leads); `/notifications.json` carries `kind":"likes"`;
  opening the topic clears it.
- [x] Green ×4, commit: `Stage 7, task 3: the bell tells the author`.

## Task 4 · The face of the forum

**Files:** `src/db/topics.rs`, `src/web/mod.rs`, `src/web/views.rs`,
`src/web/style.css`, `i18n/*.toml`, `tests/reading.rs`, `tests/face.rs` (new)

- [x] `db::topics`: `LastPost` is joined by a new `Counts { replies: i64, likes: i64, unread: bool }`
  on the row: `list_topics(db, space, category, viewer, limit, offset)`
  returns `Vec<(Topic, Option<LastPost>, Counts)>` with three subqueries:
  ```sql
  (SELECT count(*) - 1 FROM posts WHERE topic_id = t.id)                       AS replies,
  (SELECT count(*) FROM likes WHERE post_id = (SELECT min(id) FROM posts WHERE topic_id = t.id)) AS likes,
  EXISTS (SELECT 1 FROM inbox WHERE subject = ? AND topic_id = t.id AND read_at IS NULL) AS unread
  ```
  and `list_recent(db, space, viewer, limit)` — the same without the
  category condition. Callers pass `&who.subject`.
- [x] `TopicRow` gets `counts: Counts`; `views::category_index` takes
  `recent: &[TopicRow]` and renders `[ RECENT ]` under the cards as the
  same `table.threads` the section page uses, with the section's title in
  the subject's byline (slug when the configuration lost it). A
  `threads_table(lang, rows, show_section: Option<&Space>)` helper renders
  the table for both callers.
- [x] `views::section_nav(space, current: &str)` — `nav.sections` with
  `a.up href="/"` (`../` from the stylesheet, label `sections`) and one
  `a` per category, `aria-current="page"` on the current one. Rendered on
  the section page of a `topics` space, above the list.
- [x] `table.threads`: columns Topic · Replies · Likes · Last reply
  (`col_replies = "Replies"/"Antworten"`, `col_likes = "Likes"/"Gefaellt mir"`);
  the two numbers in `td.n`, mono, right-aligned, `--murmur`, `0` shown as
  `–`; `tr.unread .subject a::before` becomes `* ` in `--on` instead of `> `.
  On the phone (`max-width: 34rem`) the numbers become one inline line
  under the subject: `3 replies · 2 likes`, with `data-label` as the last
  column does today.
- [x] Topic page: `p.meta` under `h1` — section link, `N replies`
  (`replies_one/replies_many`), and the follow form in the same line,
  its button styled as a link (`.meta .follow button`); posts get
  `a.num href="#p{id}" {"#" (i+1)}` at the right end of the byline; after
  the last post `a.top href="#top"` (`top = "top"/"nach oben"`), the `<main>`
  carries `id="top"`; the reply fold gets `margin-top` and a rule above.
- [x] Timeline entries: a foot line with the heart (task 2) and
  `N comments` (`comments_one/comments_many`) as a link to `/t/{id}`.
- [x] Tests (`tests/face.rs`, route level): the front page lists the topic
  of `offtopic` under RECENT with "Off topic"; the section page carries
  `aria-current="page"` on its own link and none on the other; the table
  shows `2` under replies for a topic with two, `1` under likes after a
  like, and `class="unread"` on the row for the reader with an unread entry
  and not for the writer; the topic page carries `href="#p{id}"` with `#2`
  and `href="#top"`; the timeline entry says `2 comments` and links to the
  topic; a topic whose category is not configured still lists with its slug.
- [x] Green ×4, commit: `Stage 7, task 4: the face of the forum`.

## Task 5 · By eye, and 0.9.0

**Files:** `tests/preview.rs`, `CHANGELOG.md`, `README.md`, `Cargo.toml`,
`Cargo.lock`, `docs/plan-stage-7.md`

- [x] `tests/preview.rs`: a few likes (ben on ada's opener, ada and ben on a
  comment, ben on the blog article) so every page shows a heart in each
  state; run `cargo test --test preview -- --ignored`; open
  `target/preview/*.html` in the browser at desktop width and 390px, and
  compare with the same pages from `main` (render them from the main
  worktree into another `PREVIEW_DIR`). Fix what the eye finds, in the
  same commit.
- [x] `nix flake check` in the worktree.
- [x] `CHANGELOG.md` 0.9.0, `README.md` status, `Cargo.toml` version,
  `cargo build` for the lock; this plan's status table; commit
  `0.9.0: likes, and the face of the forum`; tag `v0.9.0` signed and pushed.
- [x] In the homeserver repository: `flake.nix` → `v0.9.0`,
  `nix flake update treff`, system build, push, `just deploy`; measure at
  the guest (`systemctl -M treff-01 show treff.service -p ExecStart` names
  the new store path; a like from a second account rings the first's bell;
  no mail in the outbox for it).
