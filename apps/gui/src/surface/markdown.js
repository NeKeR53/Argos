// Markdown minimal du composant chat des surfaces (D27), injecté dans chaque surface avec
// le SDK. Script autonome, sans dépendance ni module : il définit `argosMarkdown`.
// Reconnu : paragraphes, retours à la ligne, titres (rendus en gras), **gras**, *italique*,
// `code`, blocs ```, [liens](https://…), listes à puces ou numérotées, ![images](data:image/…).
// Tout le reste est échappé : le texte de l'agent ne devient jamais du HTML actif.

function argosMarkdown(source) {
  "use strict";

  function escape(text) {
    return text
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");
  }

  // Blocs de code d'abord : rien n'est interprété dedans. Chacun devient un bloc à part.
  var codes = [];
  var text = String(source)
    .replace(/\r\n?/g, "\n")
    .replace(/```[^\n]*\n([\s\S]*?)(?:```|$)/g, function (_, code) {
      codes.push("<pre><code>" + escape(code.replace(/\n$/, "")) + "</code></pre>");
      return "\n\n\u0000" + (codes.length - 1) + "\u0000\n\n";
    });

  // Éléments en ligne. Code, images et liens sont mis de côté avant l'échappement, puis
  // remis à leur place : gras et italique ne s'appliquent pas à leur contenu.
  function inline(raw) {
    var kept = [];
    function keep(html) {
      kept.push(html);
      return "\u0001" + (kept.length - 1) + "\u0001";
    }
    var s = raw
      .replace(/`([^`\n]+)`/g, function (_, code) {
        return keep("<code>" + escape(code) + "</code>");
      })
      .replace(/!\[([^\]\n]*)\]\((data:image\/[a-zA-Z0-9.+-]+;base64,[A-Za-z0-9+/=]+)\)/g, function (_, alt, url) {
        return keep('<img alt="' + escape(alt) + '" src="' + url + '">');
      })
      .replace(/\[([^\]\n]+)\]\(((?:https?:\/\/|mailto:)[^\s()<>"']+)\)/g, function (_, label, url) {
        return keep('<a href="' + escape(url) + '" target="_blank" rel="noopener noreferrer">' + escape(label) + "</a>");
      });
    s = escape(s)
      .replace(/\*\*([^*\n]+)\*\*/g, "<strong>$1</strong>")
      .replace(/(^|[^\w*])\*([^*\n]+)\*(?![\w*])/g, "$1<em>$2</em>")
      .replace(/(^|[^\w])_([^_\n]+)_(?!\w)/g, "$1<em>$2</em>");
    return s.replace(/\u0001(\d+)\u0001/g, function (_, index) {
      return kept[Number(index)];
    });
  }

  var BULLET = /^\s*[-*+]\s+/;
  var NUMBER = /^\s*\d+[.)]\s+/;
  var HEADING = /^\s*#{1,6}\s+/;

  // Dans un bloc (séparé par une ligne vide), les lignes consécutives de même nature
  // forment une liste ou un paragraphe.
  function block(lines) {
    var out = [];
    var i = 0;
    while (i < lines.length) {
      var line = lines[i];
      var list = BULLET.test(line) ? BULLET : NUMBER.test(line) ? NUMBER : null;
      if (list) {
        var items = [];
        while (i < lines.length && list.test(lines[i])) {
          items.push("<li>" + inline(lines[i].replace(list, "")) + "</li>");
          i++;
        }
        var tag = list === BULLET ? "ul" : "ol";
        out.push("<" + tag + ">" + items.join("") + "</" + tag + ">");
      } else if (HEADING.test(line)) {
        out.push("<p><strong>" + inline(line.replace(HEADING, "")) + "</strong></p>");
        i++;
      } else {
        var para = [];
        while (i < lines.length && !BULLET.test(lines[i]) && !NUMBER.test(lines[i]) && !HEADING.test(lines[i])) {
          para.push(inline(lines[i]));
          i++;
        }
        out.push("<p>" + para.join("<br>") + "</p>");
      }
    }
    return out.join("");
  }

  return text
    .split(/\n{2,}/)
    .map(function (chunk) {
      var trimmed = chunk.replace(/^\n+|\n+$/g, "");
      if (!trimmed.trim()) return "";
      var code = /^\u0000(\d+)\u0000$/.exec(trimmed.trim());
      return code ? codes[Number(code[1])] : block(trimmed.split("\n"));
    })
    .join("");
}
