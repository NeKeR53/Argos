// Logique de la console épurée : choix des modèles, regroupement des outils.

import { describe, expect, it } from "vitest";
import { modelChoices, modelLabel, modelTier } from "./models";
import type { ProfileInfo } from "./protocol";
import type { ThreadEntry } from "./state";
import { threadItems, toolsSummary } from "./threadView";

describe("choix du modèle", () => {
  const profiles: ProfileInfo[] = [
    {
      name: "code",
      description: "Écrire du code.",
      models: [
        { engine: "claude", model: "claude-opus-5-5" },
        { engine: "codex", model: "gpt-6.1-sol" },
      ],
    },
    {
      name: "léger",
      description: "Par défaut.",
      models: [
        { engine: "claude", model: "claude-haiku-5-5" },
        { engine: "codex", model: "gpt-6-luna" },
      ],
    },
    {
      name: "standard",
      description: "Rédaction soignée.",
      models: [
        { engine: "claude", model: "claude-sonnet-5-5" },
        { engine: "codex", model: "gpt-6.1-sol" },
      ],
    },
  ];

  it("donne des noms lisibles", () => {
    expect(modelLabel("claude-opus-5-5")).toBe("Claude Opus 5.5");
    expect(modelLabel("claude-haiku-5")).toBe("Claude Haiku 5");
    expect(modelLabel("gpt-6.1-sol")).toBe("GPT‑6.1 Sol");
    expect(modelLabel("gpt-6-luna")).toBe("GPT‑6 Luna");
    expect(modelLabel("mistral-large")).toBe("mistral-large");
  });

  it("indique une gamme quand on la connaît", () => {
    expect(modelTier("claude-haiku-5-5")).toBe("économique");
    expect(modelTier("claude-sonnet-5-5")).toBe("équilibré");
    expect(modelTier("claude-opus-5-5")).toBe("puissant");
    expect(modelTier("gpt-6-luna")).toBe("économique");
    expect(modelTier("mistral-large")).toBeNull();
  });

  it("propose chaque modèle une fois, Claude puis GPT, du plus puissant au plus économique", () => {
    expect(modelChoices(profiles).map((choice) => choice.label)).toEqual([
      "Claude Opus 5.5",
      "Claude Sonnet 5.5",
      "Claude Haiku 5.5",
      "GPT‑6.1 Sol",
      "GPT‑6 Luna",
    ]);
  });

  it("ajoute le modèle actuel de l'agent s'il n'est dans aucun profil", () => {
    const choices = modelChoices(profiles, { engine: "codex", model: "gpt-7-nova" });
    expect(choices.some((choice) => choice.model === "gpt-7-nova" && choice.engine === "codex")).toBe(true);
    expect(modelChoices(profiles, { engine: "claude", model: null })).toHaveLength(5);
  });
});

describe("outils dans la conversation", () => {
  let seq = 0;
  const entry = (value: Record<string, unknown>) => ({ seq: ++seq, ...value }) as ThreadEntry;
  const user = (text: string) => entry({ kind: "input", text, from: { kind: "user" }, console: null });
  const tool = (name: string) => entry({ kind: "tool", name });
  const reply = (text: string) => entry({ kind: "agent", text });
  const done = () => entry({ kind: "turn", outcome: { status: "completed" }, usage: null });
  const shape = (entries: ThreadEntry[], busy = false) =>
    threadItems(entries, busy).map((item) =>
      item.kind === "tools" ? { tools: item.tools.map((t) => t.name), live: item.live } : item.entry.kind,
    );

  it("regroupe les outils d'un tour en une ligne, avant la réponse", () => {
    expect(shape([user("salut"), tool("Read"), tool("Bash"), tool("Edit"), reply("fait"), done()])).toEqual([
      "input",
      { tools: ["Read", "Bash", "Edit"], live: false },
      "agent",
    ]);
  });

  it("garde un seul groupe par tour, même autour d'un message intermédiaire", () => {
    expect(shape([user("go"), tool("Read"), reply("je regarde"), tool("Bash"), reply("fini"), done()])).toEqual([
      "input",
      { tools: ["Read", "Bash"], live: false },
      "agent",
      "agent",
    ]);
  });

  it("sépare les tours, et montre en direct le groupe du tour en cours", () => {
    expect(shape([user("a"), tool("Read"), reply("ok"), done(), user("b"), tool("Bash")], true)).toEqual([
      "input",
      { tools: ["Read"], live: false },
      "agent",
      "input",
      { tools: ["Bash"], live: true },
    ]);
  });

  it("cache les fins de tour réussies et les sorties de process, pas les échecs", () => {
    const failed = entry({ kind: "turn", outcome: { status: "failed", message: "quota" }, usage: null });
    const exited = entry({ kind: "exited", code: 0 });
    expect(shape([user("a"), reply("ok"), done(), exited, failed])).toEqual(["input", "agent", "turn"]);
  });

  it("résume le nombre d'outils", () => {
    expect(toolsSummary(1)).toBe("1 outil utilisé");
    expect(toolsSummary(3)).toBe("3 outils utilisés");
  });
});
