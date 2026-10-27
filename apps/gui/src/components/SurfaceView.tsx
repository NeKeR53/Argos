// Une surface hébergée par la console (D27–D28) : iframe bac à sable, page composée par
// `buildSrcdoc`, pont `postMessage` vers le daemon. La page est rechargée à chaque version.

import { useEffect, useMemo, useRef, useState } from "react";
import type { Command, Reply, SurfaceDoc, SurfaceMeta } from "../protocol";
import type { FeedItem } from "../state";
import { feedMessage, readBridgeMessage } from "../surface/bridge";
import { buildSrcdoc, SURFACE_SANDBOX } from "../surface/srcdoc";
import { ConfirmDialog } from "./ConfirmDialog";

interface Props {
  meta: SurfaceMeta;
  /** Code déjà lu ; relu s'il est absent ou plus ancien que `meta.version`. */
  doc: SurfaceDoc | undefined;
  feed: FeedItem[];
  /** Tour en cours chez l'agent, remis à la page à son chargement. */
  busy: boolean;
  connected: boolean;
  send: (command: Command) => boolean;
  request: (command: Command) => Promise<Reply>;
}

export function SurfaceView({ meta, doc, feed, busy, connected, send, request }: Props) {
  const frame = useRef<HTMLIFrameElement>(null);
  // Dernière ligne du flux remise à la page chargée ; -1 tant que le SDK n'est pas prêt.
  const delivered = useRef(-1);
  const generation = useRef(0);
  const latest = useRef({ feed, busy });
  latest.current = { feed, busy };
  const [deleting, setDeleting] = useState(false);

  // Code à (re)lire à l'ouverture de l'onglet et à chaque nouvelle version.
  useEffect(() => {
    if (connected) send({ command: "surface", surface: meta.id });
  }, [meta.id, meta.version, connected, send]);

  const current = doc && doc.version >= meta.version ? doc : undefined;
  const srcdoc = useMemo(() => (current ? buildSrcdoc(current) : null), [current]);

  // Remet à la page les lignes qu'elle n'a pas encore.
  const flush = (withBusy: boolean) => {
    const target = frame.current?.contentWindow;
    if (!target || delivered.current < 0) return;
    const items = latest.current.feed.filter((item) => item.seq > delivered.current);
    if (items.length === 0 && !withBusy) return;
    if (items.length > 0) delivered.current = items[items.length - 1].seq;
    // Origine opaque (pas d'allow-same-origin) : « * » est la seule cible possible.
    target.postMessage(feedMessage(items, withBusy ? latest.current.busy : undefined), "*");
  };

  // Pont : seuls les messages de l'iframe de cette surface, au format du SDK.
  useEffect(() => {
    const onMessage = (event: MessageEvent) => {
      const message = readBridgeMessage(event, frame.current?.contentWindow);
      if (!message) return;
      switch (message.type) {
        case "ready":
          // Page (re)chargée : elle repart de zéro et reçoit tout le flux.
          delivered.current = 0;
          generation.current += 1;
          flush(true);
          break;
        case "send":
          send({ command: "surface_send", surface: meta.id, text: message.text });
          break;
        case "emit":
          send({ command: "surface_emit", surface: meta.id, name: message.name, payload: message.payload });
          break;
        case "call": {
          const target = frame.current?.contentWindow;
          const started = generation.current;
          const respond = (reply: { result?: unknown; error?: string }) => {
            // Une réponse d'une ancienne page ne doit pas atteindre le SDK rechargé.
            if (target && frame.current?.contentWindow === target && generation.current === started) {
              target.postMessage({ argos: 1, type: "call_result", id: message.id, ...reply }, "*");
            }
          };
          void request({
            command: "surface_call",
            surface: meta.id,
            function: message.name,
            arguments: message.arguments,
          }).then((reply) => {
            if (reply.kind === "function_result") respond({ result: reply.result });
            else respond({ error: reply.kind === "error" ? reply.message : "Réponse de fonction inattendue." });
          }, (error: unknown) => respond({ error: error instanceof Error ? error.message : "Appel de fonction impossible." }));
          break;
        }
      }
    };
    window.addEventListener("message", onMessage);
    return () => {
      generation.current += 1;
      window.removeEventListener("message", onMessage);
    };
    // `flush` ne lit que des refs : inutile de le suivre.
  }, [meta.id, send, request]);

  // Nouvelle page : rien ne lui est remis avant que son SDK annonce `ready`.
  useEffect(() => {
    delivered.current = -1;
  }, [srcdoc]);

  useEffect(() => flush(false), [feed]);

  return (
    <section className="surface" aria-label={`Surface ${meta.title}`}>
      <div className="surface-bar">
        <span className="surface-kind" data-kind={meta.kind}>
          {meta.kind}
        </span>
        <span className="surface-title">{meta.title}</span>
        <span className="muted mono">
          {meta.id} · v{meta.version}
        </span>
        <span className="spacer" />
        <button
          type="button"
          className="button button-danger-quiet"
          disabled={!connected}
          onClick={() => setDeleting(true)}
        >
          Supprimer…
        </button>
      </div>
      {srcdoc ? (
        <iframe
          // Une nouvelle version recrée l'iframe : la page repart de zéro.
          key={`${meta.id}-${current?.version}`}
          ref={frame}
          className="surface-frame"
          title={meta.title}
          sandbox={SURFACE_SANDBOX}
          srcDoc={srcdoc}
        />
      ) : (
        <p className="surface-loading muted">Chargement de la surface…</p>
      )}
      {deleting && (
        <ConfirmDialog
          title={`Supprimer « ${meta.title} » ?`}
          confirmLabel="Supprimer"
          danger
          onConfirm={() => {
            send({ command: "surface_delete", surface: meta.id });
            setDeleting(false);
          }}
          onCancel={() => setDeleting(false)}
        >
          <p>La surface est supprimée pour de bon ; l'agent pourra en créer une autre.</p>
        </ConfirmDialog>
      )}
    </section>
  );
}
