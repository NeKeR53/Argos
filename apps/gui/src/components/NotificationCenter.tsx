// Avis et erreurs de la console en toasts discrets, qui ne s'empilent pas : un seul à la
// fois, qui disparaît seul (les erreurs restent un peu plus longtemps). Une escalade n'est
// pas un avis : elle se voit par la pastille de l'agent et dans son fil.

import { useEffect, useRef } from "react";
import { Toaster, toast } from "sonner";
import type { AgentId, AgentStatus } from "../protocol";
import { displayName, type Notice } from "../state";

/** Durée d'affichage selon le ton (ms). */
const TOAST_MS: Record<Notice["tone"], number> = { info: 4000, warning: 6000, error: 8000 };

export function NotificationCenter({ notices, agents, onOpenNotice, onDismissNotice }: {
  notices: Notice[]; agents: Record<AgentId, AgentStatus>;
  onOpenNotice: (agent: AgentId) => void; onDismissNotice: (id: number) => void;
}) {
  const seen = useRef(new Set<number>());
  useEffect(() => {
    for (const item of notices) {
      if (seen.current.has(item.id)) continue;
      seen.current.add(item.id);
      const agent = item.agent && agents[item.agent];
      // Même identifiant pour tous : un nouvel avis remplace le précédent au lieu de s'empiler.
      const options = {
        id: "notice",
        duration: TOAST_MS[item.tone],
        ...(agent ? { action: { label: `Ouvrir ${displayName(agent)}`, onClick: () => onOpenNotice(agent.id) } } : {}),
        onDismiss: () => onDismissNotice(item.id),
        onAutoClose: () => onDismissNotice(item.id),
      };
      if (item.tone === "error") toast.error(item.text, options);
      else if (item.tone === "warning") toast.warning(item.text, options);
      else toast(item.text, options);
    }
  }, [notices, agents, onOpenNotice, onDismissNotice]);
  return <Toaster position="bottom-right" theme="system" visibleToasts={1} />;
}
