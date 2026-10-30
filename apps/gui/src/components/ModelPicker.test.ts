import { describe, expect, it } from "vitest";
import { modelValue, parseModelValue } from "./ModelPicker";
import { parseServerMessage } from "../protocol";
import { initialState, reduce } from "../state";

describe("choix du modèle", () => {
  it("code et relit moteur et modèle", () => {
    expect(parseModelValue(modelValue("claude", "claude-haiku-5-5"))).toEqual({
      engine: "claude",
      model: "claude-haiku-5-5",
    });
    expect(parseModelValue("gemini|x")).toBeNull();
    expect(parseModelValue("claude|")).toBeNull();
    expect(parseModelValue("")).toBeNull();
  });
});

describe("profils reçus du daemon", () => {
  it("sont lus et rangés dans l'état", () => {
    const profiles = [
      { name: "léger", description: "Par défaut", models: [{ engine: "claude", model: "claude-haiku-5-5" }] },
    ];
    const message = parseServerMessage(JSON.stringify({ type: "reply", id: 1, reply: { kind: "models", profiles, models: profiles[0].models } }));
    const state = reduce(initialState, { type: "server", message });
    expect(state.profiles?.[0].models).toEqual(profiles[0].models);
    expect(() =>
      parseServerMessage(
        JSON.stringify({ type: "reply", id: 1, reply: { kind: "models", models: [], profiles: [{ name: "x", description: "", models: [{ engine: "gemini", model: "y" }] }] } }),
      ),
    ).toThrow();
  });
});
