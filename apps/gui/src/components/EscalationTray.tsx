// Escalades ouvertes, au-dessus de la saisie (D12, D35) : l'utilisateur répond d'un clic
// sur un choix proposé, ou librement, ou écarte la question. La réponse part directement
// à l'Agent qui attend, sans passer par l'Assistant.

import { useState, type FormEvent } from "react";
import type { AgentId, Command, Escalation } from "../protocol";

interface Props {
  escalations: Escalation[];
  connected: boolean;
  send: (command: Command) => boolean;
  nameOf: (agent: AgentId) => string;
  /** Vrai dans la conversation de l'Assistant : on dit de quel Agent vient chaque question. */
  showAgent: boolean;
}

export function EscalationTray({ escalations, connected, send, nameOf, showAgent }: Props) {
  if (escalations.length === 0) return null;
  return (
    <section className="escalation-tray" aria-label="Questions en attente de votre réponse">
      {escalations.map((escalation) => (
        <EscalationCard
          key={escalation.id}
          escalation={escalation}
          connected={connected}
          send={send}
          who={showAgent ? nameOf(escalation.agent) : null}
        />
      ))}
    </section>
  );
}

function EscalationCard({
  escalation,
  connected,
  send,
  who,
}: {
  escalation: Escalation;
  connected: boolean;
  send: Props["send"];
  who: string | null;
}) {
  const [text, setText] = useState("");
  // Réponse partie, en attente de l'`escalation_updated` qui retirera la carte.
  const [sent, setSent] = useState(false);
  const answer = (value: string) => {
    const reply = value.trim();
    if (!reply || !connected) return;
    if (send({ command: "answer_escalation", escalation: escalation.id, answer: reply })) setSent(true);
  };
  const submit = (event: FormEvent) => {
    event.preventDefault();
    answer(text);
  };
  const disabled = !connected || sent;

  return (
    <article className="escalation-item" data-sent={sent || undefined}>
      <header className="escalation-item-head">
        <span className="input-tag">{who ? `${who} a besoin de vous` : "a besoin de vous"}</span>
        <button
          type="button"
          className="link escalation-dismiss"
          disabled={disabled}
          title="Écarter sans répondre : l'Agent cessera d'attendre"
          onClick={() => {
            if (send({ command: "dismiss_escalation", escalation: escalation.id })) setSent(true);
          }}
        >
          Écarter
        </button>
      </header>
      <p className="escalation-question">{escalation.question}</p>
      {escalation.options.length > 0 && (
        <div className="escalation-options">
          {escalation.options.map((option, index) => (
            <button
              key={option}
              type="button"
              className={index === 0 ? "button button-primary" : "button"}
              disabled={disabled}
              onClick={() => answer(option)}
            >
              {option}
            </button>
          ))}
        </div>
      )}
      <form className="escalation-reply" onSubmit={submit}>
        <input
          type="text"
          value={text}
          disabled={disabled}
          placeholder={escalation.options.length > 0 ? "Ou répondez librement…" : "Votre réponse…"}
          aria-label={`Réponse à : ${escalation.question}`}
          onChange={(event) => setText(event.target.value)}
        />
        <button type="submit" className="button" disabled={disabled || !text.trim()}>
          Répondre
        </button>
      </form>
    </article>
  );
}
