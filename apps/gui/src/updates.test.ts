import { describe, expect, it } from "vitest";
import { parseServerMessage } from "./protocol";
import type { UpdateStatus } from "./protocol";
import { updateView, wantsAttention } from "./updates";

const base: UpdateStatus = { current: "0.3.0", available: null, error: null, progress: null };
const release = { version: "0.4.0", notes: "Nouveautés", url: "https://github.com/NeKeR53/Argos/releases/tag/v0.4.0" };

describe("mises à jour", () => {
  it("lit la réponse du daemon", () => {
    const frame = {
      type: "reply",
      id: 3,
      reply: { kind: "update", update: { ...base, available: release, progress: { version: "0.4.0", step: "installing", message: "…", at: 1 } } },
    };
    expect(parseServerMessage(JSON.stringify(frame))).toEqual(frame);
    const odd = { ...frame, reply: { kind: "update", update: { ...base, progress: { version: "0.4.0", step: "ailleurs", message: "", at: 1 } } } };
    expect(() => parseServerMessage(JSON.stringify(odd))).toThrow();
  });

  it("propose une version plus récente", () => {
    expect(updateView(null, "0.3.0")).toEqual({ kind: "unknown" });
    expect(updateView(base, "0.3.0")).toEqual({ kind: "up_to_date", version: "0.3.0" });
    const view = updateView({ ...base, available: release }, "0.3.0");
    expect(view).toEqual({ kind: "available", ...release });
    expect(wantsAttention(view)).toBe(true);
    expect(updateView({ ...base, error: "réseau" }, null)).toEqual({ kind: "error", message: "réseau" });
  });

  it("suit l'installation puis demande de relancer l'app", () => {
    const installing = { ...base, available: release, progress: { version: "0.4.0", step: "downloading" as const, message: "téléchargement", at: 1 } };
    expect(updateView(installing, "0.3.0")).toEqual({ kind: "installing", version: "0.4.0", message: "téléchargement" });
    // Le daemon relancé est en 0.4.0, l'app tourne encore en 0.3.0.
    const done = { ...base, current: "0.4.0", progress: { version: "0.4.0", step: "done" as const, message: "fini", at: 2 } };
    expect(updateView(done, "0.3.0")).toEqual({ kind: "relaunch", version: "0.4.0", app: "0.3.0" });
    expect(updateView(done, "0.4.0")).toEqual({ kind: "up_to_date", version: "0.4.0" });
    // Dans un navigateur, pas d'app à relancer.
    expect(updateView(done, null)).toEqual({ kind: "up_to_date", version: "0.4.0" });
  });

  it("montre un échec tant que la version visée n'est pas installée", () => {
    const failed = { ...base, progress: { version: "0.4.0", step: "failed" as const, message: "somme SHA-256 incorrecte", at: 2 } };
    expect(updateView(failed, "0.3.0")).toEqual({ kind: "failed", version: "0.4.0", message: "somme SHA-256 incorrecte" });
    expect(wantsAttention(updateView(failed, "0.3.0"))).toBe(false);
    // Réessayer reste possible : la version disponible passe devant.
    expect(updateView({ ...failed, available: release }, "0.3.0").kind).toBe("available");
  });
});
