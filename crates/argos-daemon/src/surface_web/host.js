// Page hôte d'une surface ouverte dans un navigateur (surface_web.rs). Elle fait ce que
// fait `SurfaceView` dans la console : la surface tourne dans un iframe bac à sable
// (`allow-scripts` seul) composé par le daemon, et le pont `postMessage` du SDK est relayé
// vers le WebSocket de la surface. Seuls les messages de cet iframe, au format du SDK,
// sont relayés ; le daemon les revérifie.

(function () {
  "use strict";

  var surface = window.__argosHost.surface;
  var root = document.getElementById("frame");
  var status = document.getElementById("status");
  var version = document.getElementById("version");
  var frame = null;
  // Flux complet de la surface, rejoué à chaque chargement de la page de l'agent.
  var items = [];
  var busy = false;
  // Dernière ligne remise à la page chargée ; -1 tant que son SDK n'est pas prêt.
  var delivered = -1;
  // Change à chaque chargement : une réponse d'appel d'une ancienne page est jetée.
  var generation = 0;
  var calls = new Map();
  var nextCall = 1;
  var socket = null;
  var retry = 500;
  var deleted = false;

  function setStatus(text) {
    status.textContent = text;
  }

  function flush(withBusy) {
    if (!frame || !frame.contentWindow || delivered < 0) return;
    var fresh = items.filter(function (item) {
      return item.seq > delivered;
    });
    if (fresh.length === 0 && !withBusy) return;
    if (fresh.length > 0) delivered = fresh[fresh.length - 1].seq;
    var message = { argos: 1, type: "feed", items: fresh };
    if (withBusy) message.busy = busy;
    // Origine opaque (pas d'allow-same-origin) : « * » est la seule cible possible.
    frame.contentWindow.postMessage(message, "*");
  }

  function load(page) {
    document.title = page.surface.title;
    document.querySelector(".bar .title").textContent = page.surface.title;
    version.textContent = page.surface.id + " · v" + page.surface.version;
    // Une nouvelle version recrée l'iframe : la page repart de zéro.
    var next = document.createElement("iframe");
    next.className = "surface-frame";
    next.title = page.surface.title;
    next.setAttribute("sandbox", "allow-scripts");
    next.srcdoc = page.srcdoc;
    root.replaceChildren(next);
    frame = next;
    delivered = -1;
    generation += 1;
  }

  function receive(message) {
    switch (message.type) {
      case "page":
        load(message);
        break;
      case "feed": {
        var last = items.length ? items[items.length - 1].seq : 0;
        message.items.forEach(function (item) {
          if (item.seq <= last) return;
          items.push(item);
          if (item.type === "busy") busy = item.busy;
        });
        if (items.length > 500) items = items.slice(-500);
        if (typeof message.busy === "boolean") busy = message.busy;
        flush(false);
        break;
      }
      case "call_result": {
        var call = calls.get(message.id);
        if (!call) return;
        calls.delete(message.id);
        if (call.generation !== generation || !frame || !frame.contentWindow) return;
        var reply = { argos: 1, type: "call_result", id: call.id };
        if (typeof message.error === "string") reply.error = message.error;
        else reply.result = message.result;
        frame.contentWindow.postMessage(reply, "*");
        break;
      }
      case "deleted":
        deleted = true;
        frame = null;
        root.innerHTML = '<p class="muted loading">Cette surface a été supprimée.</p>';
        setStatus("");
        socket.close();
        break;
    }
  }

  function forward(message) {
    if (socket && socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message));
  }

  window.addEventListener("message", function (event) {
    if (!frame || event.source !== frame.contentWindow) return;
    var message = event.data;
    if (!message || typeof message !== "object" || message.argos !== 1) return;
    switch (message.type) {
      case "ready":
        // Page (re)chargée : elle reçoit tout le flux.
        delivered = 0;
        flush(true);
        break;
      case "send":
        if (typeof message.text === "string" && message.text.trim()) forward({ type: "send", text: message.text });
        break;
      case "emit":
        if (typeof message.name === "string" && message.name.trim()) {
          forward({ type: "emit", name: message.name, payload: message.payload === undefined ? null : message.payload });
        }
        break;
      case "call": {
        if (typeof message.id !== "number" || typeof message.name !== "string") return;
        var target = frame.contentWindow;
        if (!socket || socket.readyState !== WebSocket.OPEN) {
          target.postMessage({ argos: 1, type: "call_result", id: message.id, error: "Daemon injoignable." }, "*");
          return;
        }
        var id = nextCall++;
        calls.set(id, { id: message.id, generation: generation });
        forward({ type: "call", id: id, name: message.name, arguments: message.arguments || {} });
        break;
      }
    }
  });

  function connect() {
    socket = new WebSocket("ws://" + location.host + "/surfaces/" + encodeURIComponent(surface) + "/ws");
    socket.onopen = function () {
      // Le daemon renvoie la page et tout le flux à chaque connexion.
      items = [];
      retry = 500;
      setStatus("");
    };
    socket.onmessage = function (event) {
      try {
        receive(JSON.parse(event.data));
      } catch (error) {
        console.error("argos : message illisible du daemon", error);
      }
    };
    socket.onclose = function () {
      if (deleted) return;
      // Les appels en cours n'auront pas de réponse.
      calls.forEach(function (call) {
        if (call.generation === generation && frame && frame.contentWindow) {
          frame.contentWindow.postMessage(
            { argos: 1, type: "call_result", id: call.id, error: "Connexion au daemon perdue ; résultat de l'action incertain." },
            "*",
          );
        }
      });
      calls.clear();
      setStatus("Daemon injoignable, nouvelle tentative…");
      setTimeout(connect, retry);
      retry = Math.min(retry * 2, 10000);
    };
  }

  connect();
})();
