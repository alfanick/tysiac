(function () {
  var body = document.body;
  function resolveUrl(reference) {
    try {
      return new window.URL(reference, document.baseURI).href;
    } catch (error) {
      // The anchor fallback keeps the meta-refresh observer usable in older
      // browsers while still respecting the page's <base> element.
      var anchor = document.createElement("a");
      anchor.href = reference;
      return anchor.href;
    }
  }
  function appUrl(path) {
    return resolveUrl(String(path).replace(/^\/+/, ""));
  }
  function storageGet(name) {
    try {
      return window.localStorage.getItem(name);
    } catch (error) {
      return null;
    }
  }
  function storageSet(name, value) {
    try {
      window.localStorage.setItem(name, value);
    } catch (error) {
      // Some private-browsing and TV-browser modes deny local storage.
    }
  }
  function errorText(error, fallback) {
    if (error && typeof error.message === "string" && error.message) return error.message;
    if (typeof error === "string" && error) return error;
    return fallback;
  }
  function joinStatus(message) {
    var node = document.getElementById("join-status");
    if (node) node.textContent = " " + message;
  }

  var api = resolveUrl(body.getAttribute("data-game-api") || "game-api")
    .replace(/\/+$/, "");
  var room = body.getAttribute("data-room");
  var form = document.getElementById("join-room");

  if (form && window.fetch) {
    var rememberedCredential = storageGet("mille:" + room + ":player-password");
    if (rememberedCredential && !form.elements.password_or_token.value) {
      form.elements.password_or_token.value = rememberedCredential;
    }
    form.onsubmit = function () {
      var name = form.elements.name.value;
      var credential = form.elements.password_or_token.value;
      fetch(api + "/api/rooms/" + encodeURIComponent(room) + "/join", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name: name, password_or_token: credential })
      }).then(function (response) {
        return response.json().then(function (data) {
          if (!response.ok) throw new Error((data.message || data.code || "Join failed"));
          storageSet("mille:" + room + ":seat:" + data.seat, data.token);
          window.location.href = appUrl(data.player_url);
        });
      }).catch(function (error) {
        joinStatus(errorText(error, "Could not join the room."));
      });
      return false;
    };
  }

  // IE6 and similarly limited TV browsers keep the HTML meta refresh. Modern
  // browsers remove it and replace only the table markup, preserving anything
  // already typed into the join form.
  if (window.WebSocket && window.fetch && window.DOMParser && document.querySelectorAll) {
    var metas = document.querySelectorAll('meta[http-equiv="refresh"]');
    for (var i = 0; i < metas.length; i += 1) {
      metas[i].parentNode.removeChild(metas[i]);
    }

    var refreshPending = false;
    function refreshTable() {
      if (refreshPending) return;
      refreshPending = true;
      window.setTimeout(function () {
        fetch(window.location.pathname, { cache: "no-store" })
          .then(function (response) { return response.text(); })
          .then(function (html) {
            var parsed = new DOMParser().parseFromString(html, "text/html");
            var nextTable = parsed.getElementById("table");
            if (nextTable) document.getElementById("table").innerHTML = nextTable.innerHTML;
          })
          .catch(function () {
            // The existing table and join form remain usable while disconnected.
          })
          .then(function () { refreshPending = false; });
      }, 100);
    }

    function connect() {
      var socketUrl = api.replace(/^https:/i, "wss:").replace(/^http:/i, "ws:") +
        "/ws/" + encodeURIComponent(room) + "?role=observer";
      var ws;
      try {
        ws = new WebSocket(socketUrl);
      } catch (error) {
        joinStatus(errorText(error, "Could not open the live connection; reconnecting…"));
        window.setTimeout(connect, 2000);
        return;
      }
      ws.onmessage = function () {
        refreshTable();
      };
      ws.onerror = function () {
        joinStatus("Live connection failed; reconnecting…");
      };
      ws.onclose = function () {
        window.setTimeout(connect, 2000);
      };
    }
    connect();
  }
}());
