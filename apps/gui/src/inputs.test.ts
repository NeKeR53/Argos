import { describe, expect, it } from "vitest";
import { contextFill, engineLabel, formatUsage } from "./format";
import { classifyInput, escalationSource } from "./inputs";
import type { AgentStatus } from "./protocol";

describe("messages reçus", () => {
  it("reconnaissent les préfixes des outils", () => {
    const cases: [string, string | null, string | null, string][] = [
      ["[Mission m-1a2b3c4d de l'Assistant]\nÉclaire le salon", "mission", "Mission m-1a2b3c4d de l'Assistant", "Éclaire le salon"],
      ["[Mission m-1 annulée par l'Assistant]", "mission", "Mission m-1 annulée par l'Assistant", ""],
      ["[Tâche t-9 confiée par l'Agent « DOM » (dom)]\ninventaire", "task", "Tâche t-9 confiée par l'Agent « DOM » (dom)", "inventaire"],
      ["[Rapport de worker w1]\nfini", "report", "Rapport de worker w1", "fini"],
      ["[Rapport sur la tâche t-9 (done) de worker w1]\nfini", "report", "Rapport sur la tâche t-9 (done) de worker w1", "fini"],
      ["[Question de worker w1]\nquelle pièce ?", "question", "Question de worker w1", "quelle pièce ?"],
      ["[Argos] Ton contexte est rempli à 72 %.", "argos", "Argos", "Ton contexte est rempli à 72 %."],
    ];
    for (const [text, category, tag, body] of cases) {
      expect(classifyInput(text), text).toEqual({ category, tag, body });
    }
  });

  it("laissent libre un message sans préfixe connu", () => {
    for (const text of ["bonjour", "[note] à garder", "salut [Rapport de x]"]) {
      expect(classifyInput(text)).toEqual({ category: null, tag: null, body: text });
    }
  });

  it("lisent la source d'une escalade", () => {
    expect(escalationSource("Escalade pour l'utilisateur de l'Agent « Domotique » (7a1c-0e52)")).toEqual({
      name: "Domotique",
      agent: "7a1c-0e52",
    });
    expect(escalationSource("Escalade pour l'utilisateur de agent x")).toEqual({ name: "agent x", agent: null });
    expect(escalationSource("Rapport de worker w1")).toBeNull();
    expect(escalationSource(null)).toBeNull();
  });
});

describe("mise en forme", () => {
  const usage = { input_tokens: 24000, output_tokens: 10, cost_usd: null, context_window: 200000 };

  it("donne le remplissage du contexte quand la fenêtre est connue", () => {
    expect(contextFill(usage)).toBeCloseTo(0.12);
    expect(contextFill({ ...usage, context_window: null })).toBeNull();
    expect(contextFill({ ...usage, context_window: 0 })).toBeNull();
    expect(formatUsage(usage)).toMatch(/contexte 12\s?%$/);
    expect(formatUsage({ ...usage, context_window: null })).not.toMatch(/contexte/);
  });

  it("montre le profil à côté du moteur et du modèle", () => {
    const agent = { engine: "codex", model: "gpt-6-luna", profile: "léger" } as AgentStatus;
    expect(engineLabel(agent)).toBe("codex · gpt-6-luna (léger)");
    expect(engineLabel({ ...agent, profile: null, model: null })).toBe("codex · modèle par défaut");
  });
});
