import { describe, expect, it } from "vitest";
import type { SurfaceDoc } from "../protocol";
import { MAX_PAYLOAD, MAX_TEXT, readBridgeMessage } from "./bridge";
import markdownSource from "./markdown.js?raw";
import sdkSource from "./sdk.js?raw";
import { buildSrcdoc, SURFACE_CSP, SURFACE_SANDBOX } from "./srcdoc";

const doc: SurfaceDoc = {
  id: "s-1a2b3c4d",
  agent: "a0",
  kind: "cockpit",
  title: "Salon",
  version: 3,
  created_at: 1,
  updated_at: 2,
  html: '<div id="chat"></div><script>argos.chat(document.getElementById("chat"));</script>',
};

describe("page d'une surface", () => {
  it("pose la CSP sans réseau avant tout script", () => {
    const page = buildSrcdoc(doc);
    for (const directive of [
      "default-src 'none'",
      "script-src 'unsafe-inline'",
      "style-src 'unsafe-inline'",
      "img-src data: blob:",
      "font-src data:",
      "media-src data: blob:",
      "connect-src 'none'",
      "frame-src 'none'",
      "form-action 'none'",
    ]) {
      expect(SURFACE_CSP).toContain(directive);
    }
    const csp = page.indexOf(`<meta http-equiv="Content-Security-Policy" content="${SURFACE_CSP}">`);
    expect(csp).toBeGreaterThan(0);
    expect(csp).toBeLessThan(page.indexOf("<script"));
    expect(SURFACE_SANDBOX).toBe("allow-scripts");
  });

  it("injecte le SDK avant le HTML de l'agent, gardé tel quel", () => {
    const page = buildSrcdoc(doc);
    const sdk = page.indexOf("window.parent.postMessage");
    expect(sdk).toBeGreaterThan(0);
    expect(page.indexOf("function argosMarkdown")).toBeLessThan(sdk);
    expect(page.indexOf(doc.html)).toBeGreaterThan(sdk);
    expect(page).toContain('window.__argosSurface = {"id":"s-1a2b3c4d","title":"Salon","kind":"cockpit","version":3};');
  });

  it("échappe le titre : il ne ferme ni le titre ni le script", () => {
    const hostile = { ...doc, title: '</title></script><script>alert("x")</script> & co' };
    const page = buildSrcdoc(hostile);
    expect(page).toContain(
      "<title>&lt;/title&gt;&lt;/script&gt;&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; co</title>",
    );
    expect(page).toContain("\\u003c/script\\u003e\\u003cscript\\u003ealert");
    // Le seul `<script>alert` est absent : rien de l'agent ne s'exécute hors de son HTML.
    expect(page.split('<script>alert("x")').length).toBe(1);
  });

  it("n'a pas de `</script` dans le SDK ni le markdown", () => {
    expect(sdkSource.toLowerCase()).not.toContain("</script");
    expect(markdownSource.toLowerCase()).not.toContain("</script");
  });
});

describe("markdown du composant chat", () => {
  const render = new Function(`${markdownSource}\nreturn argosMarkdown;`)() as (source: string) => string;

  it("rend paragraphes, gras, italique, code et listes", () => {
    expect(render("Bonjour **toi**, *vraiment* et `x < y`.\nSuite")).toBe(
      "<p>Bonjour <strong>toi</strong>, <em>vraiment</em> et <code>x &lt; y</code>.<br>Suite</p>",
    );
    expect(render("- un\n- deux\n\n1. premier\n2. second")).toBe(
      "<ul><li>un</li><li>deux</li></ul><ol><li>premier</li><li>second</li></ol>",
    );
    expect(render("```js\nconst a = '<b>';\n```\nfin")).toBe(
      "<pre><code>const a = &#39;&lt;b&gt;&#39;;</code></pre><p>fin</p>",
    );
  });

  it("n'accepte que des liens web et des images en data:", () => {
    expect(render("[doc](https://example.org/a?b=1&c=2)")).toBe(
      '<p><a href="https://example.org/a?b=1&amp;c=2" target="_blank" rel="noopener noreferrer">doc</a></p>',
    );
    expect(render("[x](javascript:alert(1))")).toBe("<p>[x](javascript:alert(1))</p>");
    expect(render("![pt](data:image/png;base64,iVBORw0KGgo=)")).toBe(
      '<p><img alt="pt" src="data:image/png;base64,iVBORw0KGgo="></p>',
    );
    expect(render("![pt](https://example.org/a.png)")).toBe(
      '<p>!<a href="https://example.org/a.png" target="_blank" rel="noopener noreferrer">pt</a></p>',
    );
  });

  it("échappe tout HTML", () => {
    expect(render('<img src=x onerror="alert(1)">')).toBe("<p>&lt;img src=x onerror=&quot;alert(1)&quot;&gt;</p>");
  });
});

describe("pont des surfaces", () => {
  const frame = {} as Window;
  const from = (data: unknown, source: unknown = frame) => readBridgeMessage({ source, data }, frame);

  it("ignore une source inconnue", () => {
    expect(from({ argos: 1, type: "send", text: "salut" }, {} as Window)).toBeNull();
    expect(from({ argos: 1, type: "send", text: "salut" }, null)).toBeNull();
    expect(readBridgeMessage({ source: frame, data: { argos: 1, type: "ready" } }, null)).toBeNull();
  });

  it("n'accepte que le format du SDK", () => {
    expect(from({ argos: 1, type: "ready" })).toEqual({ type: "ready" });
    expect(from({ argos: 1, type: "send", text: "salut" })).toEqual({ type: "send", text: "salut" });
    expect(from({ argos: 1, type: "emit", name: "allumer", payload: { piece: "salon" } })).toEqual({
      type: "emit",
      name: "allumer",
      payload: { piece: "salon" },
    });
    for (const data of [
      "texte",
      null,
      [],
      { type: "send", text: "sans étiquette" },
      { argos: 2, type: "send", text: "autre version" },
      { argos: 1, type: "tree" },
      { argos: 1, type: "send", text: "   " },
      { argos: 1, type: "send", text: "x".repeat(MAX_TEXT + 1) },
      { argos: 1, type: "emit", name: "", payload: 1 },
      { argos: 1, type: "emit", name: "gros", payload: "x".repeat(MAX_PAYLOAD) },
      { argos: 1, type: "emit", name: "bigint", payload: 10n },
    ]) {
      expect(from(data), JSON.stringify(data, (_, v) => (typeof v === "bigint" ? "bigint" : v))).toBeNull();
    }
  });

  it("ramène une charge utile à du JSON", () => {
    expect(from({ argos: 1, type: "emit", name: "e", payload: undefined })).toEqual({
      type: "emit",
      name: "e",
      payload: null,
    });
    expect(from({ argos: 1, type: "emit", name: "e", payload: { a: undefined, b: [1, "2"] } })).toEqual({
      type: "emit",
      name: "e",
      payload: { b: [1, "2"] },
    });
  });
});
