// Completing @handle — the one script treff ships (docs/decisions/0005).
//
// Typing `@` in a text field opens a list of everybody who may be mentioned
// in this space; typing on after the `@` narrows it. Nothing depends on this
// file: without it, `@handle` is typed by hand and works the same.
//
// The list comes from `/mentionable`, once per page, on the first `@`. Who is
// in it is decided on the server by the same rule that decides who is told
// (`mentions::offered`) — this file only shows what it was given.
//
// No library, no inline code, and no `innerHTML`: every name reaches the page
// as text.
(function () {
  "use strict";

  // The boundary rule of `markup::mention_spans`: an `@` counts at the start
  // or after a character that could not be part of an address.
  var PART_OF_ADDRESS = /[\p{L}\p{N}._%+-]/u;
  // What may follow the `@` while completing. Wider than a handle on purpose:
  // people type the NAME as often as the handle ("@mü" for Konrad Müller),
  // and what is written in the end is always the handle.
  var QUERY_CHAR = /[\p{L}\p{N}._-]/u;

  var list = null; // a Promise of [{name, handle}], or of null after a failure
  var counter = 0;

  function load() {
    if (list === null) {
      list = fetch("/mentionable", {
        credentials: "same-origin",
        headers: { Accept: "application/json" },
      })
        .then(function (r) {
          return r.ok ? r.json() : null;
        })
        .catch(function () {
          return null;
        });
    }
    return list;
  }

  // Where the `@` of the word at the caret is, and what follows it — or null
  // when the caret is not in a mention.
  function atCaret(field) {
    var end = field.selectionStart;
    if (end !== field.selectionEnd) return null;
    var text = field.value;
    var i = end;
    while (i > 0 && QUERY_CHAR.test(text[i - 1])) i--;
    if (i === 0 || text[i - 1] !== "@") return null;
    var at = i - 1;
    if (at > 0 && PART_OF_ADDRESS.test(text[at - 1])) return null;
    var query = text.slice(i, end);
    if (query.length > 64) return null;
    return { at: at, end: end, query: query.toLowerCase() };
  }

  // WHERE the `@` sits inside the field, in pixels from the field's own
  // upper left corner — so the list opens under the character somebody just
  // typed and not under the whole box (Achim, 2026-09-22).
  //
  // A textarea offers no such measurement: it has a caret and no way to ask
  // where it is. So the text up to the `@` is laid out A SECOND TIME in a
  // hidden element with the same typography and the same width, and the
  // marker at its end is measured. The copied properties are the ones that
  // move a line break: everything about the font, the padding, the border and
  // the wrapping. Miss one and the mirror wraps elsewhere than the field.
  var MIRRORED = [
    "fontFamily", "fontSize", "fontWeight", "fontStyle", "fontVariant",
    "letterSpacing", "lineHeight", "textTransform", "textIndent",
    "wordSpacing", "tabSize", "whiteSpace", "wordBreak", "overflowWrap",
    "paddingTop", "paddingRight", "paddingBottom", "paddingLeft",
    "borderTopWidth", "borderRightWidth", "borderBottomWidth",
    "borderLeftWidth",
  ];

  function caretSpot(field, index) {
    var host = field.parentNode;
    if (!host || !field.offsetParent) return null;
    var style = window.getComputedStyle(field);
    var mirror = document.createElement("div");
    for (var k = 0; k < MIRRORED.length; k++) {
      mirror.style[MIRRORED[k]] = style[MIRRORED[k]];
    }
    mirror.style.position = "absolute";
    mirror.style.visibility = "hidden";
    mirror.style.boxSizing = "border-box";
    mirror.style.width = field.offsetWidth + "px";
    mirror.style.height = "auto";
    mirror.style.top = "0";
    mirror.style.left = "0";
    // A textarea wraps and keeps its spaces whatever `white-space` says.
    mirror.style.whiteSpace = "pre-wrap";
    mirror.style.overflowWrap = "break-word";
    mirror.textContent = field.value.slice(0, index);
    var marker = document.createElement("span");
    // Not empty: a zero-width box has no place on a line of its own.
    marker.textContent = "@";
    mirror.appendChild(marker);
    host.appendChild(mirror);
    var line = parseFloat(style.lineHeight);
    if (!(line > 0)) line = parseFloat(style.fontSize) * 1.2;
    var spot = {
      top: field.offsetTop + marker.offsetTop - field.scrollTop + line,
      left: field.offsetLeft + marker.offsetLeft - field.scrollLeft,
    };
    host.removeChild(mirror);
    return spot;
  }

  // Beginning of the handle, or of any word of the name.
  function matches(person, query) {
    if (query === "") return true;
    if (person.handle.indexOf(query) === 0) return true;
    return person.name
      .toLowerCase()
      .split(/\s+/)
      .some(function (word) {
        return word.indexOf(query) === 0;
      });
  }

  function attach(field) {
    var id = "mention-list-" + ++counter;
    var box = document.createElement("ul");
    box.id = id;
    box.className = "mention-list";
    box.setAttribute("role", "listbox");
    box.hidden = true;
    // A frame of its own around the field, so the list hangs from the
    // field's lower edge and not from wherever the form's layout puts it.
    var host = document.createElement("span");
    host.className = "mention-host";
    field.parentNode.insertBefore(host, field);
    host.appendChild(field);
    host.appendChild(box);
    field.setAttribute("aria-autocomplete", "list");
    field.setAttribute("aria-controls", id);
    field.setAttribute("aria-expanded", "false");

    var shown = []; // the people currently in the box
    var active = 0;
    var spot = null; // the result of atCaret while the box is open

    function close() {
      box.hidden = true;
      box.style.removeProperty("--at-top");
      box.style.removeProperty("--at-left");
      shown = [];
      spot = null;
      field.setAttribute("aria-expanded", "false");
      field.removeAttribute("aria-activedescendant");
    }

    function highlight(index) {
      active = index;
      var items = box.children;
      for (var k = 0; k < items.length; k++) {
        items[k].setAttribute("aria-selected", k === index ? "true" : "false");
      }
      if (items[index]) {
        field.setAttribute("aria-activedescendant", items[index].id);
        items[index].scrollIntoView({ block: "nearest" });
      }
    }

    function take(person) {
      if (!spot) return;
      var text = field.value;
      var insert = "@" + person.handle + " ";
      field.value = text.slice(0, spot.at) + insert + text.slice(spot.end);
      var caret = spot.at + insert.length;
      field.setSelectionRange(caret, caret);
      close();
      field.focus();
    }

    function show(people) {
      while (box.firstChild) box.removeChild(box.firstChild);
      shown = people;
      people.forEach(function (person, k) {
        var item = document.createElement("li");
        item.id = id + "-" + k;
        item.setAttribute("role", "option");
        var name = document.createElement("span");
        name.className = "name";
        name.textContent = person.name;
        var handle = document.createElement("span");
        handle.className = "handle";
        handle.textContent = "@" + person.handle;
        item.appendChild(name);
        item.appendChild(handle);
        // `mousedown`, not `click`: a click would take the focus away from
        // the field first, and with it the caret the handle belongs at.
        item.addEventListener("mousedown", function (event) {
          event.preventDefault();
          take(person);
        });
        box.appendChild(item);
      });
      // Unter das `@`, nicht unter das Feld. Nur die zwei gemessenen Zahlen
      // gehen ins Element; die Regel, was damit geschieht, steht im
      // Stylesheet (`--at-top`/`--at-left`, mit dem alten Verhalten als
      // Rueckfall). Misslingt die Messung, bleibt es beim Rueckfall.
      var at = spot ? caretSpot(field, spot.at) : null;
      if (at) {
        box.style.setProperty("--at-top", at.top + "px");
        box.style.setProperty("--at-left", at.left + "px");
      } else {
        box.style.removeProperty("--at-top");
        box.style.removeProperty("--at-left");
      }
      box.hidden = false;
      field.setAttribute("aria-expanded", "true");
      highlight(0);
    }

    function update() {
      var found = atCaret(field);
      if (!found) {
        close();
        return;
      }
      load().then(function (people) {
        // The field may have moved on while the list was loading.
        var now = atCaret(field);
        if (!people || !now || now.at !== found.at) {
          if (!now) close();
          return;
        }
        var fitting = people.filter(function (p) {
          return matches(p, now.query);
        });
        if (fitting.length === 0) {
          close();
          return;
        }
        spot = now;
        show(fitting);
      });
    }

    field.addEventListener("input", update);
    field.addEventListener("click", update);
    field.addEventListener("keydown", function (event) {
      if (box.hidden || shown.length === 0) return;
      if (event.key === "ArrowDown") {
        event.preventDefault();
        highlight((active + 1) % shown.length);
      } else if (event.key === "ArrowUp") {
        event.preventDefault();
        highlight((active - 1 + shown.length) % shown.length);
      } else if (event.key === "Enter" || event.key === "Tab") {
        event.preventDefault();
        take(shown[active]);
      } else if (event.key === "Escape") {
        event.preventDefault();
        close();
      }
    });
    // Moving the caret sideways is no `input`, and the list must follow it.
    field.addEventListener("keyup", function (event) {
      if (["ArrowLeft", "ArrowRight", "Home", "End"].indexOf(event.key) >= 0) update();
    });
    field.addEventListener("blur", close);
  }

  function start() {
    var fields = document.querySelectorAll("form textarea");
    for (var k = 0; k < fields.length; k++) attach(fields[k]);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", start);
  } else {
    start();
  }
})();
