// Mises à jour d'Argos (D46) : ce que la console en montre. Le daemon cherche et installe ;
// la console propose, suit l'avancement et relance l'app une fois remplacée.

import { useCallback, useEffect, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import type { Command, Reply, UpdateStatus } from "./protocol";

/** Recherche de fond : l'API GitHub limite les appels, le daemon en garde une heure. */
const CHECK_EVERY_MS = 6 * 3600_000;
/** Relecture de l'avancement pendant une installation. */
const PROGRESS_EVERY_MS = 2_000;

export type UpdateView =
  | { kind: "unknown" }
  | { kind: "error"; message: string }
  | { kind: "up_to_date"; version: string }
  | { kind: "available"; version: string; notes: string; url: string }
  | { kind: "installing"; version: string; message: string }
  | { kind: "failed"; version: string; message: string }
  | { kind: "relaunch"; version: string; app: string };

/**
 * Ce qu'il faut montrer. `app` est la version de l'app (Tauri) ; hors de l'app, `null`.
 * Une installation récente prime : son avancement, puis son échec. Un daemon plus récent
 * que l'app veut dire que l'app a été remplacée sur le disque mais tourne encore.
 */
export function updateView(status: UpdateStatus | null, app: string | null): UpdateView {
  if (!status) return { kind: "unknown" };
  const progress = status.progress;
  if (progress && progress.step !== "done" && progress.step !== "failed") {
    return { kind: "installing", version: progress.version, message: progress.message };
  }
  if (app && app !== status.current) return { kind: "relaunch", version: status.current, app };
  if (status.available) return { kind: "available", ...status.available };
  if (progress?.step === "failed" && progress.version !== status.current) {
    return { kind: "failed", version: progress.version, message: progress.message };
  }
  if (status.error) return { kind: "error", message: status.error };
  return { kind: "up_to_date", version: status.current };
}

/** La console doit-elle attirer l'œil (pastille de la barre latérale) ? */
export function wantsAttention(view: UpdateView): boolean {
  return view.kind === "available" || view.kind === "relaunch" || view.kind === "installing";
}

export interface Updates {
  view: UpdateView;
  checking: boolean;
  check: () => void;
  apply: () => void;
  relaunch: () => void;
}

export function useUpdates(connected: boolean, request: (command: Command) => Promise<Reply>): Updates {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [app, setApp] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);

  useEffect(() => {
    if (!isTauri()) return;
    void invoke<{ version: string }>("core_info").then((info) => setApp(info.version), () => undefined);
  }, []);

  const read = useCallback(
    (refresh: boolean) =>
      request({ command: "check_update", refresh }).then(
        (reply) => {
          if (reply.kind === "update") setStatus(reply.update);
        },
        () => undefined,
      ),
    [request],
  );

  useEffect(() => {
    if (!connected) return;
    void read(false);
    const timer = setInterval(() => void read(false), CHECK_EVERY_MS);
    return () => clearInterval(timer);
  }, [connected, read]);

  const view = updateView(status, app);
  // Le daemon est relancé en cours de route : la connexion tombe puis revient, et la
  // relecture reprend avec elle.
  useEffect(() => {
    if (!connected || view.kind !== "installing") return;
    const timer = setInterval(() => void read(false), PROGRESS_EVERY_MS);
    return () => clearInterval(timer);
  }, [connected, view.kind, read]);

  const check = useCallback(() => {
    setChecking(true);
    void read(true).finally(() => setChecking(false));
  }, [read]);
  const apply = useCallback(() => {
    void request({ command: "apply_update" }).then(() => read(false), () => undefined);
  }, [request, read]);
  const relaunch = useCallback(() => {
    if (isTauri()) void invoke("restart_app");
  }, []);

  return { view, checking, check, apply, relaunch };
}
