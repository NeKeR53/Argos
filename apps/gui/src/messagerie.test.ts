import { describe, expect, it } from "vitest";
import { hueOf, initials } from "./components/Avatar";
import { preview } from "./components/ConversationList";
import type { AgentStatus } from "./protocol";

const agent = (over: Partial<AgentStatus> = {}) =>
  ({ id: "a1", level: "agent", busy: false, running: false, charter: { name: "Courrier Alexandre", scope: "Trie la boîte perso" }, ...over }) as AgentStatus;

describe("liste des conversations", () => {
  it("donne des initiales et une teinte stables", () => {
    expect(initials("Courrier Alexandre")).toBe("CA");
    expect(initials("maison")).toBe("MA");
    expect(initials("entretien-mac")).toBe("EM");
    expect(hueOf("Maison")).toBe(hueOf("Maison"));
  });

  it("aperçoit le dernier message, sinon le rôle", () => {
    expect(preview(agent(), [])).toBe("Trie la boîte perso");
    expect(preview(agent(), [{ seq: 1, kind: "agent", text: "Deux mails de la banque" }])).toBe("Deux mails de la banque");
    expect(
      preview(agent(), [{ seq: 1, kind: "input", text: "Trie", from: null, console: null }]),
    ).toBe("Vous : Trie");
    expect(preview(agent({ busy: true }), [{ seq: 2, kind: "tool", name: "Bash" }])).toBe("Bash…");
  });
});
