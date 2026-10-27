// SDK Argos des surfaces (D27), injecté avant le HTML de l'agent dans chaque surface.
// Script autonome, sans dépendance. Il expose `window.argos` :
//   argos.surface            { id, title, kind, version } de cette surface
//   argos.send(text)         texte remis à l'agent (préfixé par le daemon)
//   argos.emit(name, data)   événement structuré remis à l'agent
//   argos.call(name, args)   Promise du résultat d'une fonction, sans réveiller l'agent
//   argos.on(type, fn)       'data' (données du cockpit), 'message' (messages de l'agent),
//                            'busy' (tour en cours) ; rend une fonction de désabonnement
//   argos.chat(element)      composant chat standard dans `element`
// La surface tourne dans un iframe bac à sable sans réseau (D28) ; ni formulaire soumis,
// ni popup, ni accès à la console. Tout passe par
// `postMessage` avec la console, qui vérifie l'émetteur. Format des messages :
//   surface → console  { argos: 1, type: "ready" | "send" | "emit" | "call", … }
//   console → surface  { argos: 1, type: "feed" | "call_result", … }
// `argosMarkdown` (markdown.js) est défini juste avant ce script.

(function () {
  "use strict";

  var meta = window.__argosSurface || {};
  var listeners = { data: [], message: [], busy: [], sent: [] };
  // Tout ce que la console a remis depuis le chargement : un abonné tardif le reçoit aussi.
  var history = [];
  var busy = false;
  var nextCallId = 1;
  var pending = new Map();

  function post(message) {
    message.argos = 1;
    // Origine opaque de part et d'autre (iframe sans allow-same-origin) : « * » est la
    // seule cible possible ; la console vérifie l'émetteur de son côté.
    window.parent.postMessage(message, "*");
  }

  function call(fn, value) {
    try {
      fn(value);
    } catch (error) {
      console.error("argos : erreur dans un abonné", error);
    }
  }

  function receive(item) {
    if (!item || typeof item.type !== "string") return;
    if (item.type === "busy") {
      busy = Boolean(item.busy);
      listeners.busy.forEach(function (fn) {
        call(fn, busy);
      });
      return;
    }
    var value = item.type === "data" ? item.data : item.text;
    if (!listeners[item.type]) return;
    history.push({ type: item.type, value: value });
    listeners[item.type].forEach(function (fn) {
      call(fn, value);
    });
  }

  window.addEventListener("message", function (event) {
    if (event.source !== window.parent) return;
    var message = event.data;
    if (message && message.argos === 1 && message.type === "call_result") {
      var entry = pending.get(message.id);
      if (!entry) return;
      if (typeof message.error !== "string" && !Object.prototype.hasOwnProperty.call(message, "result")) return;
      pending.delete(message.id);
      clearTimeout(entry.timer);
      if (typeof message.error === "string") entry.reject(new Error(message.error));
      else entry.resolve(message.result);
      return;
    }
    if (!message || message.argos !== 1 || message.type !== "feed" || !Array.isArray(message.items)) return;
    message.items.forEach(receive);
    if (typeof message.busy === "boolean") receive({ type: "busy", busy: message.busy });
  });

  function on(type, fn) {
    if (!listeners[type] || typeof fn !== "function") {
      throw new Error("argos.on : type inconnu « " + type + " » (data, message, busy)");
    }
    listeners[type].push(fn);
    // Rattrapage : ce qui est arrivé avant l'abonnement.
    if (type === "busy") {
      call(fn, busy);
    } else {
      history.forEach(function (entry) {
        if (entry.type === type) call(fn, entry.value);
      });
    }
    return function off() {
      listeners[type] = listeners[type].filter(function (other) {
        return other !== fn;
      });
    };
  }

  function send(text) {
    text = String(text == null ? "" : text);
    if (!text.trim()) return;
    post({ type: "send", text: text });
  }

  function emit(name, payload) {
    // Normalisé en JSON : seules des données simples traversent le pont.
    var data = payload === undefined ? null : JSON.parse(JSON.stringify(payload));
    post({ type: "emit", name: String(name), payload: data });
  }

  function callFunction(name, args) {
    return new Promise(function (resolve, reject) {
      if (typeof name !== "string" || !/^[a-z][a-z0-9_]{0,63}$/.test(name)) {
        reject(new Error("argos.call : nom de fonction invalide"));
        return;
      }
      if (pending.size >= 32) {
        reject(new Error("argos.call : trop d'appels en cours"));
        return;
      }
      var data;
      try {
        var encoded = JSON.stringify(args === undefined ? {} : args);
        if (typeof encoded !== "string" || new TextEncoder().encode(encoded).byteLength > 64 * 1024) throw new Error();
        data = JSON.parse(encoded);
        if (!data || typeof data !== "object" || Array.isArray(data)) throw new Error();
      } catch (_) {
        reject(new Error("argos.call : objet JSON attendu (64 Kio au plus)"));
        return;
      }
      var id = nextCallId++;
      var timer = setTimeout(function () {
        pending.delete(id);
        reject(new Error("argos.call : délai dépassé ; résultat de l'action incertain"));
      }, 36000);
      pending.set(id, { resolve: resolve, reject: reject, timer: timer });
      post({ type: "call", id: id, name: name, arguments: data });
    });
  }

  window.addEventListener("pagehide", function () {
    pending.forEach(function (entry) {
      clearTimeout(entry.timer);
      entry.reject(new Error("argos.call : surface fermée ; résultat de l'action incertain"));
    });
    pending.clear();
  });

  var STYLE =
    ".argos-chat{display:flex;flex-direction:column;height:100%;min-height:240px;font:14px/1.45 -apple-system,BlinkMacSystemFont,'Segoe UI',system-ui,sans-serif;color:#1c1c1a;background:#f6f6f4;border:1px solid #e2e2de;border-radius:8px;overflow:hidden}" +
    ".argos-chat-log{flex:1;overflow-y:auto;display:flex;flex-direction:column;gap:6px;padding:12px}" +
    ".argos-chat-msg{max-width:85%;padding:7px 10px;border-radius:10px;overflow-wrap:anywhere}" +
    ".argos-chat-msg p{margin:0 0 6px}.argos-chat-msg p:last-child{margin-bottom:0}" +
    ".argos-chat-msg ul,.argos-chat-msg ol{margin:0 0 6px;padding-left:20px}" +
    ".argos-chat-msg pre{margin:0 0 6px;padding:8px;border-radius:6px;background:rgba(0,0,0,.06);overflow-x:auto}" +
    ".argos-chat-msg code{font:12.5px ui-monospace,'SF Mono',Menlo,monospace}" +
    ".argos-chat-msg img{max-width:100%;border-radius:6px}" +
    ".argos-chat-msg a{color:#3d54c9}" +
    ".argos-chat-sent{align-self:flex-end;white-space:pre-wrap;background:#e8ebfa}" +
    ".argos-chat-agent{align-self:flex-start;background:#fff;border:1px solid #e2e2de}" +
    ".argos-chat-busy{align-self:flex-start;color:#6d6d68;font-size:12px;padding:0 4px}" +
    ".argos-chat-form{display:flex;gap:8px;padding:10px;border-top:1px solid #e2e2de;background:#fff}" +
    ".argos-chat-form textarea{flex:1;min-height:40px;max-height:160px;padding:7px 9px;border:1px solid #cfcfca;border-radius:6px;font:inherit;color:inherit;background:#f6f6f4;resize:vertical}" +
    ".argos-chat-form button{padding:0 14px;border:1px solid #3d54c9;border-radius:6px;background:#3d54c9;color:#fff;font:inherit;cursor:pointer}" +
    ".argos-chat-form button:disabled{opacity:.45;cursor:default}" +
    "@media (prefers-color-scheme:dark){.argos-chat{color:#e9e9ec;background:#161618;border-color:#2f2f34}" +
    ".argos-chat-sent{background:#262c4d}.argos-chat-agent{background:#1d1d20;border-color:#2f2f34}" +
    ".argos-chat-msg pre{background:rgba(255,255,255,.07)}.argos-chat-msg a{color:#8396ff}" +
    ".argos-chat-form{background:#1d1d20;border-color:#2f2f34}" +
    ".argos-chat-form textarea{background:#161618;border-color:#3d3d43}" +
    ".argos-chat-form button{background:#8396ff;border-color:#8396ff;color:#11131f}}";

  var styled = false;

  // Composant chat standard : le fil (envois depuis cette surface et messages de l'agent,
  // en markdown) et une saisie. Le texte seulement : le daemon ne transmet pas d'image.
  function chat(element) {
    if (!(element instanceof Element)) throw new Error("argos.chat : élément attendu");
    if (!styled) {
      var style = document.createElement("style");
      style.textContent = STYLE;
      document.head.appendChild(style);
      styled = true;
    }
    element.innerHTML = "";
    var root = document.createElement("div");
    root.className = "argos-chat";
    var log = document.createElement("div");
    log.className = "argos-chat-log";
    log.setAttribute("role", "log");
    var waiting = document.createElement("div");
    waiting.className = "argos-chat-busy";
    waiting.textContent = "L'agent travaille…";
    waiting.hidden = true;
    // Pas de <form> : le bac à sable (sans allow-forms) bloque toute soumission, même
    // interceptée. Le bouton et la touche Entrée appellent directement l'envoi.
    var form = document.createElement("div");
    form.className = "argos-chat-form";
    var input = document.createElement("textarea");
    input.placeholder = "Message à l'agent…";
    input.setAttribute("aria-label", "Message à l'agent");
    var button = document.createElement("button");
    button.type = "button";
    button.textContent = "Envoyer";
    form.appendChild(input);
    form.appendChild(button);
    log.appendChild(waiting);
    root.appendChild(log);
    root.appendChild(form);
    element.appendChild(root);

    function add(className, html) {
      var nearBottom = log.scrollHeight - log.scrollTop - log.clientHeight < 40;
      var bubble = document.createElement("div");
      bubble.className = "argos-chat-msg " + className;
      bubble.innerHTML = html;
      log.insertBefore(bubble, waiting);
      if (nearBottom) log.scrollTop = log.scrollHeight;
    }

    function submit(event) {
      if (event) event.preventDefault();
      var text = input.value.trim();
      if (!text) return;
      send(text);
      input.value = "";
    }

    button.addEventListener("click", submit);
    input.addEventListener("keydown", function (event) {
      if (event.key === "Enter" && !event.shiftKey && !event.isComposing) submit(event);
    });

    var offs = [
      on("sent", function (text) {
        add("argos-chat-sent", escapeText(text));
      }),
      on("message", function (text) {
        add("argos-chat-agent", argosMarkdown(text));
      }),
      on("busy", function (value) {
        waiting.hidden = !value;
      }),
    ];
    return {
      destroy: function () {
        offs.forEach(function (off) {
          off();
        });
        element.innerHTML = "";
      },
    };
  }

  function escapeText(text) {
    var div = document.createElement("div");
    div.textContent = text;
    return div.innerHTML;
  }

  var api = {
    surface: Object.freeze({
      id: meta.id,
      title: meta.title,
      kind: meta.kind,
      version: meta.version,
    }),
    send: send,
    emit: emit,
    call: callFunction,
    on: function (type, fn) {
      if (type === "sent") throw new Error("argos.on : type inconnu « sent » (data, message, busy)");
      return on(type, fn);
    },
    chat: chat,
  };
  Object.defineProperty(window, "argos", { value: Object.freeze(api), enumerable: true });
  delete window.__argosSurface;

  // La console répond par le flux complet de la surface.
  post({ type: "ready" });
})();
