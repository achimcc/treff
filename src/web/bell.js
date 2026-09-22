// The bell: an overlay with the entries, and a number that stays live —
// the second script treff ships (docs/decisions/0005, amended).
//
// Without this file the bell is a link to the list, and the number is what
// it was when the page was made. With it, a click opens the list in place;
// a click on an entry goes to it, and "all notifications" goes to the page.
//
// Everything it needs is on the bell itself: `data-stream` (Server-Sent
// Events), `data-json` (the same, once), `href` (the page), and its words as
// `data-t-*` in the page's language. That is why the start page can use this
// same file.
//
// No library, no inline code, no `innerHTML`: every title and name reaches
// the page as text.
(function () {
  "use strict";

  // Wherever the tag stands, the bell must be there first: a page that loads
  // this without `defer` would otherwise find nothing and do nothing.
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", start);
  } else {
    start();
  }

  function start() {
  var bell = document.querySelector("a.bell[data-stream]");
  if (!bell) return;

  function t(key) {
    return bell.getAttribute("data-t-" + key) || "";
  }

  var host = document.createElement("span");
  host.className = "bell-host";
  bell.parentNode.insertBefore(host, bell);
  host.appendChild(bell);

  var panel = document.createElement("div");
  panel.className = "bell-panel";
  panel.id = "bell-panel";
  panel.setAttribute("role", "dialog");
  panel.setAttribute("aria-label", t("title"));
  panel.hidden = true;
  var list = document.createElement("ul");
  list.className = "bell-list";
  var all = document.createElement("a");
  all.className = "bell-all";
  all.href = bell.href;
  all.textContent = t("all");
  panel.appendChild(list);
  panel.appendChild(all);
  host.appendChild(panel);

  bell.setAttribute("aria-haspopup", "dialog");
  bell.setAttribute("aria-controls", panel.id);
  bell.setAttribute("aria-expanded", "false");

  var latest = null; // the last bell received: {unread, entries}

  function el(tag, cls, text) {
    var node = document.createElement(tag);
    if (cls) node.className = cls;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  function when(seconds) {
    try {
      return new Date(seconds * 1000).toLocaleString(document.documentElement.lang || undefined, {
        dateStyle: "short",
        timeStyle: "short",
      });
    } catch (e) {
      return "";
    }
  }

  function line(entry) {
    var item = el("li", entry.unread ? "new" : "seen");
    var link = el("a");
    link.href = entry.link;
    var sub = el("span", "byline");
    if (entry.kind === "replies") {
      var key = entry.unread
        ? entry.count === 1 ? "new-reply-in" : "new-replies-in"
        : entry.count === 1 ? "reply-in" : "replies-in";
      link.appendChild(document.createTextNode(entry.count + " " + t(key) + " "));
      link.appendChild(el("b", "", entry.title));
      sub.textContent = t("latest-from") + " " + entry.author + " · " + when(entry.at);
    } else if (entry.kind === "mention") {
      link.appendChild(document.createTextNode(entry.author + " " + t("mentioned-you-in") + " "));
      link.appendChild(el("b", "", entry.title));
      sub.textContent = when(entry.at);
    } else if (entry.kind === "film_available" || entry.kind === "film_failed") {
      if (entry.kind === "film_available") link.appendChild(document.createTextNode("🎬 "));
      link.appendChild(el("b", "", entry.title));
      link.appendChild(
        document.createTextNode(" " + t(entry.kind === "film_available" ? "film-available" : "film-failed"))
      );
      sub.textContent = (entry.reason ? entry.reason + " · " : "") + when(entry.at);
    } else {
      return null; // a kind this page does not know is left out, not guessed
    }
    item.appendChild(link);
    item.appendChild(sub);
    return item;
  }

  function render(data) {
    latest = data;
    var badge = bell.querySelector(".unread");
    if (data.unread > 0) {
      if (!badge) {
        badge = el("span", "unread");
        bell.appendChild(badge);
      }
      badge.textContent = String(data.unread);
      bell.setAttribute("aria-label", t("title") + ": " + data.unread + " " + t("unread"));
      bell.classList.add("ringing");
    } else {
      if (badge) badge.parentNode.removeChild(badge);
      bell.setAttribute("aria-label", t("title"));
      bell.classList.remove("ringing");
    }
    while (list.firstChild) list.removeChild(list.firstChild);
    var shown = 0;
    (data.entries || []).forEach(function (entry) {
      var item = line(entry);
      if (item) {
        list.appendChild(item);
        shown++;
      }
    });
    if (shown === 0) list.appendChild(el("li", "empty", t("nothing")));
  }

  function load() {
    fetch(bell.getAttribute("data-json"), { credentials: "same-origin", headers: { Accept: "application/json" } })
      .then(function (r) {
        return r.ok ? r.json() : null;
      })
      .then(function (data) {
        if (data) render(data);
      })
      .catch(function () {});
  }

  function open() {
    if (!latest) load();
    panel.hidden = false;
    bell.setAttribute("aria-expanded", "true");
  }

  function close() {
    panel.hidden = true;
    bell.setAttribute("aria-expanded", "false");
  }

  bell.addEventListener("click", function (event) {
    // A click with a modifier is a request for the page itself, in a new tab.
    if (event.ctrlKey || event.metaKey || event.shiftKey || event.button !== 0) return;
    event.preventDefault();
    if (panel.hidden) open();
    else close();
  });
  document.addEventListener("click", function (event) {
    if (!panel.hidden && !host.contains(event.target)) close();
  });
  document.addEventListener("keydown", function (event) {
    if (event.key === "Escape" && !panel.hidden) {
      close();
      bell.focus();
    }
  });

  if (typeof EventSource === "function") {
    var source = new EventSource(bell.getAttribute("data-stream"), { withCredentials: true });
    source.addEventListener("bell", function (event) {
      try {
        render(JSON.parse(event.data));
      } catch (e) {
        // A frame that is not a bell changes nothing.
      }
    });
  }
  }
})();
