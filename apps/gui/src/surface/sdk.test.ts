import { afterEach, describe, expect, it, vi } from "vitest";
import sdkSource from "./sdk.js?raw";

type SDK = { call: (name: string, args?: unknown) => Promise<unknown>; send: (text: string) => void };

function surface() {
  const messages: Record<string, unknown>[] = [];
  const listeners: Record<string, ((event: { source?: unknown; data?: unknown }) => void)[]> = {};
  const parent = { postMessage: (message: Record<string, unknown>) => messages.push(message) };
  const window = {
    parent,
    argos: undefined as SDK | undefined,
    addEventListener: (type: string, listener: (event: { source?: unknown; data?: unknown }) => void) => {
      (listeners[type] ??= []).push(listener);
    },
  };
  new Function("window", sdkSource)(window);
  return {
    api: window.argos!, messages,
    reply: (data: unknown, source: unknown = parent) => listeners.message.forEach((listener) => listener({ source, data })),
    close: () => listeners.pagehide.forEach((listener) => listener({})),
  };
}

afterEach(() => vi.useRealTimers());

describe("argos.call", () => {
  it("corrèle les résultats même quand ils reviennent dans le désordre", async () => {
    const page = surface();
    const first = page.api.call("set_light", { name: "salon", state: true });
    const second = page.api.call("read_light");
    expect(page.messages).toEqual([
      { argos: 1, type: "ready" },
      { argos: 1, type: "call", id: 1, name: "set_light", arguments: { name: "salon", state: true } },
      { argos: 1, type: "call", id: 2, name: "read_light", arguments: {} },
    ]);
    // Le SDK ignore les autres fenêtres et les réponses sans résultat ni erreur.
    page.reply({ argos: 1, type: "call_result", id: 1, error: "faux" }, {});
    page.reply({ argos: 1, type: "call_result", id: 1 });
    page.reply({ argos: 1, type: "call_result", id: 2, result: false });
    page.reply({ argos: 1, type: "call_result", id: 1, result: { state: true } });
    expect(await second).toBe(false);
    expect(await first).toEqual({ state: true });
    page.api.send("toujours disponible");
    expect(page.messages[page.messages.length - 1]).toEqual({ argos: 1, type: "send", text: "toujours disponible" });
  });

  it("remet les erreurs au code de la surface", async () => {
    const page = surface();
    const pending = page.api.call("missing");
    page.reply({ argos: 1, type: "call_result", id: 1, error: "Fonction inconnue." });
    await expect(pending).rejects.toThrow("Fonction inconnue.");
  });

  it("refuse les noms et arguments invalides avant l'envoi", async () => {
    const page = surface();
    for (const name of ["", "Set_light", "autre.set_light", "a".repeat(65)]) {
      await expect(page.api.call(name)).rejects.toThrow("nom de fonction invalide");
    }
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    for (const args of [null, [], true, 1n, cyclic, { huge: "é".repeat(32768) }]) {
      await expect(page.api.call("set_light", args)).rejects.toThrow("objet JSON attendu");
    }
    expect(page.messages).toEqual([{ argos: 1, type: "ready" }]);
  });

  it("borne les appels simultanés et rejette ceux d'une surface fermée", async () => {
    vi.useFakeTimers();
    const page = surface();
    const calls = Array.from({ length: 32 }, () => page.api.call("read_light").catch((error: Error) => error.message));
    await expect(page.api.call("read_light")).rejects.toThrow("trop d'appels");
    page.close();
    expect((await Promise.all(calls)).every((message) => typeof message === "string" && message.includes("surface fermée"))).toBe(true);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("rejette une réponse absente sans renvoyer l'action", async () => {
    vi.useFakeTimers();
    const page = surface();
    const call = page.api.call("set_light").catch((error: Error) => error.message);
    await vi.advanceTimersByTimeAsync(36_000);
    expect(await call).toContain("résultat de l'action incertain");
    expect(page.messages.filter((message) => message.type === "call")).toHaveLength(1);
  });
});
