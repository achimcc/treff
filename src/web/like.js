// The heart without a reload — the third script treff ships, on the terms
// of ADR 0005 and ADR 0008.
//
// Without this file a like is a form: the page comes back at the post with
// the new number. With it, the click is sent with `fetch`, and the button's
// state and number change in place.
//
// TWO KINDS OF FAILURE, AND THEY MUST NOT BE TREATED ALIKE. The route is a
// toggle. If the request never reached the server (offline) or the server
// refused it (not `ok`), nothing was changed, and the form goes the
// ordinary way so the page can say what happened. But if the server said
// `ok` and only the answer came back broken — the connection dropped after
// the commit, a proxy mangled the body — the like IS on, and submitting the
// form again would take it back. Then the true state is READ (`GET`, never
// a toggle) and shown; if even that fails, the button is left as it was and
// the next page load tells the truth.
//
// No library, no inline code, no `innerHTML`.
(function () {
  "use strict";

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", start);
  } else {
    start();
  }

  function start() {
    var forms = document.querySelectorAll("form.like");
    Array.prototype.forEach.call(forms, function (form) {
      form.addEventListener("submit", function (event) {
        var button = form.querySelector("button[type=submit]");
        if (!button || !window.fetch) return; // the browser submits
        event.preventDefault();
        button.disabled = true;
        var action = form.getAttribute("action");
        var reached = false; // did the server answer `ok`?
        fetch(action, {
          method: "POST",
          credentials: "same-origin",
          headers: { Accept: "application/json" },
        })
          .then(function (response) {
            if (!response.ok) throw new Error("not ok");
            reached = true;
            return response.json();
          })
          .then(function (data) {
            show(button, data);
            button.disabled = false;
          })
          .catch(function () {
            if (!reached) {
              // Nothing was changed: let the page say what happened.
              button.disabled = false;
              form.submit();
              return;
            }
            // The like is on (or off) — read which, never toggle again.
            fetch(action, { credentials: "same-origin", headers: { Accept: "application/json" } })
              .then(function (response) {
                if (!response.ok) throw new Error("not ok");
                return response.json();
              })
              .then(function (data) {
                show(button, data);
              })
              .catch(function () {
                // Unknown state: leave the button as it was; the next page
                // load tells the truth. Nothing is toggled twice.
              })
              .then(function () {
                button.disabled = false;
              });
          });
      });
    });
  }

  // The words for both states travel on the button itself, so the file
  // carries no translation of its own. `aria-label` is what a screen reader
  // hears INSTEAD of the button's content — the action of the next click
  // and the number, like the server writes it: "Unlike, 2 likes".
  function show(button, data) {
    var liked = data.liked === true;
    var n = Number(data.count) || 0;
    button.setAttribute("aria-pressed", liked ? "true" : "false");
    var action = button.getAttribute(liked ? "data-t-unlike" : "data-t-like");
    if (action) {
      var label = action;
      if (n === 1 && button.getAttribute("data-t-likes-one")) {
        label += ", " + button.getAttribute("data-t-likes-one");
      } else if (n > 1 && button.getAttribute("data-t-likes-many")) {
        label += ", " + n + " " + button.getAttribute("data-t-likes-many");
      }
      button.setAttribute("aria-label", label);
    }
    var count = button.querySelector(".count");
    if (n > 0) {
      if (!count) {
        count = document.createElement("span");
        count.className = "count";
        button.appendChild(count);
      }
      count.textContent = String(n);
    } else if (count) {
      count.parentNode.removeChild(count);
    }
  }
})();
