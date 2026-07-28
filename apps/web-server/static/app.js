(function () {
  "use strict";
  var body = document.body;
  var page = body.dataset.page;
  var api = new URL(body.dataset.gameApi || "game-api", document.baseURI)
    .href.replace(/\/+$/, "");
  var sameOriginApi = new URL("game-api", document.baseURI).href.replace(/\/+$/, "");
  var room = body.dataset.room;
  var statusNode = document.getElementById("status");
  var currentView = null;
  var selectedCard = null;
  var giftSelections = {};
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
      // Private browsing and hardened policies may disable persistent storage.
    }
  }
  function errorText(error, fallback) {
    if (error && typeof error.message === "string" && error.message) return error.message;
    if (typeof error === "string" && error) return error;
    return fallback;
  }
  function appUrl(path) {
    return new URL(String(path).replace(/^\/+/, ""), document.baseURI).href;
  }
  var dictionaries = {
    en:{pass:"Pass",continue_after_talon:"Continue",surrender:"Surrender",claim_remaining:"Claim all remaining",
      proof:"Proof response",reveal_proof:"Reveal proof",contract:"Contract",bid:"Bid",claim_vote:"Claim vote",
      play:"Play selected card",request_proof:"Request proof",waive_proof:"No proof needed",
      accept_claim:"Accept claim",reject_claim:"Reject claim",sending:"Sending…"},
    de:{pass:"Passen",continue_after_talon:"Weiter",surrender:"Aufgeben",claim_remaining:"Restliche Stiche beanspruchen",
      proof:"Nachweis",reveal_proof:"Nachweis zeigen",contract:"Ansage",bid:"Bieten",claim_vote:"Anspruch abstimmen",
      play:"Gewählte Karte spielen",request_proof:"Nachweis verlangen",waive_proof:"Kein Nachweis",
      accept_claim:"Anspruch annehmen",reject_claim:"Anspruch ablehnen",sending:"Wird gesendet…"},
    pl:{pass:"Pas",continue_after_talon:"Dalej",surrender:"Poddać się",claim_remaining:"Zgłoś wszystkie pozostałe lewy",
      proof:"Sprawdzenie meldunku",reveal_proof:"Pokaż meldunek",contract:"Gra",bid:"Licytuj",claim_vote:"Głosuj",
      play:"Zagraj wybraną kartę",request_proof:"Zażądaj pokazania",waive_proof:"Bez pokazywania",
      accept_claim:"Uznaj",reject_claim:"Odrzuć",sending:"Wysyłanie…"}
  };
  var detected = (navigator.language || "en").slice(0, 2).toLowerCase();
  var locale = storageGet("mille:locale") || (dictionaries[detected] ? detected : "en");
  function tr(key) { return (dictionaries[locale] && dictionaries[locale][key]) || dictionaries.en[key] || key; }

  function status(text) { if (statusNode) statusNode.textContent = text || ""; }
  function key(suffix) { return "mille:" + room + ":" + suffix; }
  function commandId() {
    if (window.crypto && crypto.randomUUID) return crypto.randomUUID();
    return Date.now() + "-" + Math.random().toString(16).slice(2);
  }
  function request(url, options) {
    return fetch(api + url, options).then(function (response) {
      return response.json().then(function (value) {
        if (!response.ok || value.type === "error") {
          throw new Error(value.message || value.code || "Request failed");
        }
        return value;
      });
    });
  }
  function requestWithoutResponse(url, options) {
    return fetch(sameOriginApi + url, options).then(function (response) {
      if (response.ok) return;
      return response.json().then(function (value) {
        throw new Error(value.message || value.code || "Request failed");
      }, function () {
        throw new Error("Request failed");
      });
    });
  }
  function esc(value) {
    return String(value == null ? "" : value).replace(/[&<>"']/g, function (c) {
      return {"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c];
    });
  }
  function suitSymbol(suit) {
    return {clubs:"♣",diamonds:"♦",hearts:"♥",spades:"♠"}[suit] || "";
  }
  function rankLabel(rank) {
    return {nine:"9",ten:"10",jack:"J",queen:"Q",king:"K",ace:"A"}[rank] || rank;
  }
  function cardKey(card) { return card.rank + ":" + card.suit; }
  function cardHtml(card, selectable) {
    var red = card.suit === "hearts" || card.suit === "diamonds";
    var selected = selectedCard === cardKey(card);
    return '<button class="card ' + (red ? "red " : "") + (selected ? "selected" : "") +
      '" ' + (selectable ? 'data-card="' + esc(cardKey(card)) + '"' : "disabled") + ">" +
      "<b>" + rankLabel(card.rank) + "</b><span>" + suitSymbol(card.suit) + "</span></button>";
  }
  function publicState(view) {
    var html = "";
    if (view.presentation && view.presentation.stage !== "ready") {
      html += "<p>♠ " + esc(view.presentation.stage) + " · " +
        view.presentation.visible_deal_cards + "/24</p><div class=\"table-center\">";
      for (var back = 0; back < view.presentation.visible_deal_cards; back += 1) {
        html += '<span class="card back">♠</span>';
      }
      html += "</div>";
    }
    html += '<div class="scoreboard">';
    var game = view.game;
    (view.players || []).forEach(function (player) {
      var turn = game && game.turn && game.turn === player.seat;
      html += '<div class="player-tile ' + (turn ? "turn" : "") + '"><b>' + esc(player.name) +
        "</b><br>" + player.score + " · " + player.card_count + " cards" +
        (player.connected ? " ●" : " ○") + "</div>";
    });
    html += '</div><div class="table-center">';
    if (game) {
      (game.current_trick || []).forEach(function (played) { html += cardHtml(played.card, false); });
      html += "</div><p>Phase: <b>" + esc(game.phase) + "</b> · Bid/contract: <b>" +
        esc(game.bid_or_contract || "—") + "</b> · Trump: <b>" + suitSymbol(game.trump) + "</b></p>";
    } else {
      html += "<p>Waiting for three players. The game starts automatically.</p></div>";
    }
    html += '<div class="log">' + (view.history || []).slice(-10).reverse().map(function (event) {
      return "<div>" + esc(event.message) + "</div>";
    }).join("") + "</div>";
    return html;
  }

  function openSocket(role, seat, credential) {
    var url = api.replace(/^https:/i, "wss:").replace(/^http:/i, "ws:") +
      "/ws/" + encodeURIComponent(room) +
      "?role=" + role + (seat == null ? "" : "&seat=" + seat) +
      (credential ? "&credential=" + encodeURIComponent(credential) : "");
    var ws;
    try {
      ws = new WebSocket(url);
    } catch (error) {
      status(errorText(error, "Could not open the game connection; reconnecting…"));
      window.setTimeout(function () { openSocket(role, seat, credential); }, 1800);
      return;
    }
    ws.onmessage = function (event) {
      try {
        var message = JSON.parse(event.data);
        if (message.type === "snapshot") show(message[role] || message);
        if (message.type === "updated") show(message.view);
        if (message.type === "error") status(message.message);
      } catch (error) {
        status(errorText(error, "The game server sent an invalid update."));
      }
    };
    ws.onerror = function () {
      status("Game connection error; reconnecting…");
    };
    ws.onclose = function () {
      status("Disconnected; reconnecting…");
      window.setTimeout(function () { openSocket(role, seat, credential); }, 1800);
    };
    window.milleSocket = ws;
  }

  function show(view) {
    currentView = view;
    var publicView = view.public || view.observer || view;
    document.getElementById("view").innerHTML = publicState(publicView);
    if (page === "referee") {
      document.getElementById("raw").textContent = JSON.stringify(view.state || view, null, 2);
      return;
    }
    if (page !== "player") return;
    var playable = {};
    (view.legal_actions || []).filter(function (action) { return action.type === "play_card"; })
      .forEach(function (action) { playable[cardKey(action.card)] = action; });
    document.getElementById("hand").innerHTML = (view.own_hand || []).map(function (card) {
      return cardHtml(card, !!playable[cardKey(card)]);
    }).join("");
    Array.prototype.forEach.call(document.querySelectorAll("[data-card]"), function (node) {
      node.onclick = function () { selectedCard = node.dataset.card; show(view); };
    });
    renderActions(view, playable);
  }

  function renderActions(view, playable) {
    var node = document.getElementById("actions");
    var actions = view.legal_actions || [];
    var html = "";
    if (selectedCard && playable[selectedCard]) html += '<button id="play-selected">' + esc(tr("play")) + '</button>';
    actions.forEach(function (action, index) {
      if (action.type === "play_card" || action.type === "transfer") return;
      var label = {
        pass:tr("pass"), continue_after_talon:tr("continue_after_talon"), surrender:tr("surrender"),
        claim_remaining:tr("claim_remaining"), respond_to_proof:tr("proof"),
        reveal_proof:tr("reveal_proof"), confirm_contract:tr("contract"), bid:tr("bid"),
        vote_on_claim:tr("claim_vote")
      }[action.type] || action.type;
      if (action.points) label += " " + action.points;
      if (action.reveal != null) label = action.reveal ? tr("request_proof") : tr("waive_proof");
      if (action.accept != null) label = action.accept ? tr("accept_claim") : tr("reject_claim");
      if (action.suits) label += " " + action.suits.map(suitSymbol).join("+");
      html += '<button data-action="' + index + '">' + esc(label) + "</button>";
    });
    var transfers = actions.filter(function (action) { return action.type === "transfer"; });
    if (transfers.length) {
      var recipients = transfers[0].gifts.map(function (gift) { return gift.recipient; });
      html += '<p>Select one card for each recipient, then confirm:</p>';
      (view.own_hand || []).forEach(function (card) {
        recipients.forEach(function (recipient) {
          var chosen = giftSelections[recipient] === cardKey(card);
          html += '<button class="' + (chosen ? "chosen-gift" : "") +
            '" data-gift-card="' + esc(cardKey(card)) + '" data-recipient="' + recipient + '">' +
            rankLabel(card.rank) + suitSymbol(card.suit) + " → " + (recipient + 1) + "</button>";
        });
      });
      var selectedTransfer = transfers.filter(function (candidate) {
        return candidate.gifts.every(function (gift) {
          return giftSelections[gift.recipient] === cardKey(gift.card);
        });
      })[0];
      if (selectedTransfer) html += '<button id="confirm-transfer">Confirm gifts</button>';
    }
    node.innerHTML = html;
    if (document.getElementById("play-selected")) {
      document.getElementById("play-selected").onclick = function () { sendAction(playable[selectedCard]); };
    }
    Array.prototype.forEach.call(node.querySelectorAll("[data-action]"), function (button) {
      button.onclick = function () { sendAction(actions[Number(button.dataset.action)]); };
    });
    Array.prototype.forEach.call(node.querySelectorAll("[data-gift-card]"), function (button) {
      button.onclick = function () {
        Object.keys(giftSelections).forEach(function (recipient) {
          if (giftSelections[recipient] === button.dataset.giftCard) delete giftSelections[recipient];
        });
        giftSelections[button.dataset.recipient] = button.dataset.giftCard;
        renderActions(view, playable);
      };
    });
    if (document.getElementById("confirm-transfer")) {
      document.getElementById("confirm-transfer").onclick = function () {
        giftSelections = {};
        sendAction(selectedTransfer);
      };
    }
  }

  function sendAction(action) {
    var seat = Number(body.dataset.seat);
    if (!window.milleSocket || window.milleSocket.readyState !== WebSocket.OPEN) {
      status("The controlling connection is not ready.");
      return;
    }
    status(tr("sending"));
    window.milleSocket.send(JSON.stringify({
      type:"act", command_id:commandId(),
      expected_revision:(currentView.public || currentView).revision, action:action
    }));
    selectedCard = null;
  }

  if (page === "lobby") {
    Array.prototype.forEach.call(document.querySelectorAll("[data-delete-room]"), function (button) {
      button.onclick = function () {
        var roomName = button.dataset.room;
        if (!window.confirm('Remove room "' + roomName + '"?')) return;
        button.disabled = true;
        status('Removing room "' + roomName + '"…');
        requestWithoutResponse("/api/rooms/" + encodeURIComponent(roomName), {method:"DELETE"})
          .then(function () {
            var row = button.closest("li");
            if (row) row.remove();
            else window.location.reload();
            status('Removed room "' + roomName + '".');
          })
          .catch(function (error) {
            button.disabled = false;
            status(errorText(error, "Could not remove the room."));
          });
      };
    });
    document.getElementById("create-room").onsubmit = function (event) {
      event.preventDefault();
      var form = event.target;
      var bodyValue = {
        name:form.name.value, player_password:form.player_password.value,
        referee_password:form.referee_password.value, seed:form.seed.value || null,
        config:{target_score:Number(form.target_score.value), lock_score:Number(form.lock_score.value),
          talon_visibility:form.talon_visibility.value}
      };
      request("/api/rooms", {method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(bodyValue)})
        .then(function (created) {
          storageSet("mille:" + created.name + ":referee", created.referee_password);
          storageSet("mille:" + created.name + ":player-password", created.player_password);
          window.location.href = appUrl(created.observer_url);
        }).catch(function (error) { status(errorText(error, "Could not create the room.")); });
    };
  }

  var localeSelect = document.getElementById("locale");
  if (localeSelect) {
    localeSelect.value = locale;
    localeSelect.onchange = function () {
      locale = localeSelect.value;
      storageSet("mille:locale", locale);
      if (currentView) show(currentView);
    };
  }

  if (page === "player") {
    var seat = Number(body.dataset.seat);
    var tokenKey = key("seat:" + seat);
    var credential = storageGet(tokenKey) || storageGet(key("player-password")) || "";
    document.getElementById("credential").value = credential;
    document.getElementById("connect").onclick = function () {
      var entered = document.getElementById("credential").value;
      request("/api/rooms/" + encodeURIComponent(room) + "/join", {
        method:"POST",headers:{"Content-Type":"application/json"},
        body:JSON.stringify({name:body.dataset.name,password_or_token:entered})
      }).then(function (joined) {
        storageSet("mille:" + room + ":seat:" + joined.seat, joined.token);
        if (joined.seat !== seat) { window.location.href = appUrl(joined.player_url); return; }
        document.getElementById("auth").classList.add("hidden");
        openSocket("player", seat, joined.token);
      }).catch(function (error) { status(errorText(error, "Could not join the room.")); });
    };
    if (credential) document.getElementById("connect").click();
  }

  if (page === "referee") {
    var refKey = key("referee");
    document.getElementById("credential").value = storageGet(refKey) || "";
    document.getElementById("connect").onclick = function () {
      var credential = document.getElementById("credential").value;
      storageSet(refKey, credential);
      request("/api/rooms/" + encodeURIComponent(room) + "/view?role=referee&credential=" + encodeURIComponent(credential))
        .then(function (view) {
          document.getElementById("auth").classList.add("hidden");
          document.getElementById("admin").classList.remove("hidden");
          show(view); openSocket("referee", null, credential);
        }).catch(function (error) { status(errorText(error, "Could not open the referee view.")); });
    };
    Array.prototype.forEach.call(document.querySelectorAll("[data-admin]"), function (button) {
      button.onclick = function () {
        var publicView = currentView.public || currentView;
        request("/api/rooms/" + encodeURIComponent(room) + "/admin", {
          method:"POST",headers:{"Content-Type":"application/json"},
          body:JSON.stringify({referee_password:storageGet(refKey),command_id:commandId(),
            expected_revision:publicView.revision,action:button.dataset.admin})
        }).then(function (message) { show(message.view); })
          .catch(function (error) { status(errorText(error, "The referee action failed.")); });
      };
    });
    if (document.getElementById("credential").value) document.getElementById("connect").click();
  }
}());
