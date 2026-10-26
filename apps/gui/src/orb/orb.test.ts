import { describe, expect, it } from "vitest";
import { maskedWebhookUrl, webhookUrl } from "../hooks";
import { formatInterval } from "../components/TriggerPanel";
import { orbMode, PANIC_WINDOW_MS, pressPanic, readVoiceState, SILENT } from "./logic";

describe("états de l'orbe", () => {
  it("suit l'Assistant tant que la voix ne dit rien", () => {
    expect(orbMode(true, false, SILENT)).toBe("idle");
    expect(orbMode(true, true, SILENT)).toBe("thinking");
    expect(orbMode(false, false, SILENT)).toBe("offline");
  });

  it("laisse la main à la voix quand elle écoute ou parle", () => {
    expect(orbMode(true, true, { state: "listening", level: 0.4 })).toBe("listening");
    expect(orbMode(false, false, { state: "speaking", level: 0.9 })).toBe("speaking");
  });

  it("lit la charge de voice-state en bornant le niveau", () => {
    expect(readVoiceState({ state: "listening", level: 1.7 })).toEqual({ state: "listening", level: 1 });
    expect(readVoiceState({ state: "speaking", level: Number.NaN })).toEqual({ state: "speaking", level: 0 });
    expect(readVoiceState({ state: "chante", level: 0.5 })).toEqual({ state: null, level: 0.5 });
    expect(readVoiceState(null)).toEqual(SILENT);
  });
});

describe("panic stop global", () => {
  it("part au second appui dans les 2 s", () => {
    const first = pressPanic(null, 1000);
    expect(first).toEqual({ fire: false, armedAt: 1000 });
    expect(pressPanic(first.armedAt, 1000 + PANIC_WINDOW_MS)).toEqual({ fire: true, armedAt: null });
  });

  it("réarme après un second appui trop tardif", () => {
    expect(pressPanic(1000, 1000 + PANIC_WINDOW_MS + 1)).toEqual({ fire: false, armedAt: 3001 });
  });

  it("ne part pas deux fois de suite sur un troisième appui", () => {
    const second = pressPanic(pressPanic(null, 0).armedAt, 500);
    expect(second.fire).toBe(true);
    expect(pressPanic(second.armedAt, 800)).toEqual({ fire: false, armedAt: 800 });
  });
});

describe("déclencheurs : affichage", () => {
  it("compose l'URL d'un webhook, secret masqué à l'affichage", () => {
    expect(webhookUrl("http://127.0.0.1:7879/", "tr-1a2b", "s3cr3t")).toBe("http://127.0.0.1:7879/hooks/tr-1a2b/s3cr3t");
    expect(maskedWebhookUrl("http://127.0.0.1:7879", "tr-1a2b")).toBe("http://127.0.0.1:7879/hooks/tr-1a2b/••••••••");
  });

  it("écrit un intervalle en clair", () => {
    expect(formatInterval("900")).toBe("toutes les 15 min");
    expect(formatInterval("7200")).toBe("toutes les 2 h");
    expect(formatInterval("90")).toBe("toutes les 90 s");
    expect(formatInterval("bof")).toBe("bof");
  });
});
