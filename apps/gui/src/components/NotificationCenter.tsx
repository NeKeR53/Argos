import { useEffect, useRef } from "react";
import { Toaster, toast } from "sonner";
import type { AgentId, AgentStatus } from "../protocol";
import { displayName, type Escalation, type Notice } from "../state";

export function NotificationCenter({ notices, escalations, agents, onOpenNotice, onOpenEscalation, onDismissNotice, onDismissEscalation }: {
  notices: Notice[]; escalations: Escalation[]; agents: Record<AgentId, AgentStatus>;
  onOpenNotice: (agent: AgentId) => void; onOpenEscalation: (escalation: Escalation) => void;
  onDismissNotice: (id: number) => void; onDismissEscalation: (id: number) => void;
}) {
  const seen = useRef(new Set<string>());
  useEffect(() => {
    for (const item of escalations) {
      const key = `escalation-${item.id}`;
      if (seen.current.has(key)) continue;
      seen.current.add(key);
      toast.warning(`L'Agent ${item.name} a besoin de vous`, {
        id: key, description: item.text, duration: Infinity,
        action: { label: "Ouvrir", onClick: () => onOpenEscalation(item) },
        onDismiss: () => onDismissEscalation(item.id),
      });
    }
    for (const item of notices) {
      const key = `notice-${item.id}`;
      if (seen.current.has(key)) continue;
      seen.current.add(key);
      const agent = item.agent && agents[item.agent];
      const options = {
        id: key,
        ...(agent ? { action: { label: `Ouvrir ${displayName(agent)}`, onClick: () => onOpenNotice(agent.id) } } : {}),
        onDismiss: () => onDismissNotice(item.id),
        onAutoClose: () => onDismissNotice(item.id),
      };
      if (item.tone === "error") toast.error(item.text, options);
      else if (item.tone === "warning") toast.warning(item.text, options);
      else toast(item.text, options);
    }
  }, [notices, escalations, agents, onOpenNotice, onOpenEscalation, onDismissNotice, onDismissEscalation]);
  return <Toaster position="bottom-right" theme="system" visibleToasts={3} closeButton />;
}
