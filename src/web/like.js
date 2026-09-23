// The heart without a reload — the third script treff ships, on the terms
// of ADR 0005 and ADR 0008.
//
// Without this file a like is a form: the page comes back at the post with
// the new number. With it, the click is sent with `fetch`, and the button's
// state and number change in place. Anything that is not a plain success —
// offline, a 5xx, a session that ended — hands the form back to the browser
// to submit the ordinary way, so nothing is ever lost quietly.
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
        fetch(form.getAttribute("action"), {
          method: "POST",
          credentials: "same-origin",
          headers: { Accept: "application/json" },
        })
          .then(function (response) {
            if (!response.ok) throw new Error("not ok");
            return response.json();
          })
          .then(function (data) {
            show(button, data);
            button.disabled = false;
          })
          .catch(function () {
            // Let the page say what happened: the ordinary submit.
            button.disabled = false;
            form.submit();
          });
      });
    });
  }

  // The words for both states travel on the button itself, so the file
  // carries no translation of its own. `aria-label` is the label of the
  // NEXT click; `aria-pressed` the state a screen reader hears.
  function show(button, data) {
    var liked = data.liked === true;
    button.setAttribute("aria-pressed", liked ? "true" : "false");
    var words = button.getAttribute(liked ? "data-t-unlike" : "data-t-like");
    if (words) button.setAttribute("aria-label", words);
    var count = button.querySelector(".count");
    var n = Number(data.count) || 0;
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
