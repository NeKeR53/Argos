// Board d'un Agent, en lecture seule : missions ouvertes et tâches (état, Worker, résultat).
// Relu à l'ouverture de l'Agent et après chaque événement qui le concerne, lui ou l'un
// de ses Workers (rafales regroupées).

import { useEffect } from "react";
import type { AgentId, AgentStatus, Board, Command, Task } from "../protocol";
import { shortId } from "../state";
import { taskStateLabel } from "../format";

/** Délai qui regroupe une rafale d'événements en une seule lecture du board. */
const REFRESH_DELAY_MS = 250;

interface Props {
  agent: AgentStatus;
  board: Board | undefined;
  /** Change à chaque événement qui touche ce board. */
  revision: number;
  connected: boolean;
  open: boolean;
  onToggle: (open: boolean) => void;
  send: (command: Command) => boolean;
  /** Vrai si l'agent est encore dans l'arbre (un Worker fini peut avoir été retiré). */
  isLiving: (agent: AgentId) => boolean;
  onSelect: (agent: AgentId) => void;
}

export function BoardSection(props: Props) {
  const { agent, board, revision, connected, open, onToggle, send } = props;
  useEffect(() => {
    if (!connected) return;
    const timer = setTimeout(() => send({ command: "board", agent: agent.id }), REFRESH_DELAY_MS);
    return () => clearTimeout(timer);
  }, [agent.id, revision, connected, send]);

  const missions = board?.missions ?? [];
  const tasks = board?.tasks ?? [];
  const loose = tasks.filter((task) => task.mission === null);

  return (
    <details className="board" open={open} onToggle={(event) => onToggle(event.currentTarget.open)}>
      <summary>
        <span className="board-title">Board</span>
        <span className="muted">
          {board ? `${plural(missions.length, "mission ouverte", "missions ouvertes")} · ${plural(tasks.length, "tâche", "tâches")}` : "lecture…"}
        </span>
      </summary>
      <div className="board-body">
        {board && missions.length === 0 && tasks.length === 0 && <p className="muted">Board vide.</p>}
        {missions.map((mission) => (
          <section key={mission.id} className="board-mission">
            <header>
              <span className="mono muted">{mission.id}</span>
              <span className="board-mission-text">{mission.text}</span>
            </header>
            <TaskList tasks={tasks.filter((task) => task.mission === mission.id)} isLiving={props.isLiving} onSelect={props.onSelect} />
          </section>
        ))}
        {loose.length > 0 && (
          <section className="board-mission">
            <header>
              <span className="muted">Hors mission</span>
            </header>
            <TaskList tasks={loose} isLiving={props.isLiving} onSelect={props.onSelect} />
          </section>
        )}
      </div>
    </details>
  );
}

function TaskList({ tasks, isLiving, onSelect }: { tasks: Task[] } & Pick<Props, "isLiving" | "onSelect">) {
  if (tasks.length === 0) return <p className="board-empty muted">Pas encore de tâche : l'Agent planifie.</p>;
  return (
    <ul className="board-tasks">
      {tasks.map((task) => (
        <li key={task.id} className="board-task">
          <span className="task-state" data-state={task.state}>
            {taskStateLabel[task.state]}
          </span>
          <div className="board-task-text">
            <span>{task.title}</span>
            {task.result && <span className="board-task-result muted">{task.result}</span>}
          </div>
          {task.worker && !isLiving(task.worker) && (
            <span className="mono muted board-task-worker" title={`Worker ${task.worker} (retiré)`}>
              {shortId(task.worker)}
            </span>
          )}
          {task.worker && isLiving(task.worker) && (
            <button
              type="button"
              className="link mono board-task-worker"
              onClick={() => onSelect(task.worker!)}
              title={`Ouvrir le Worker ${task.worker}`}
            >
              {shortId(task.worker)}
            </button>
          )}
        </li>
      ))}
    </ul>
  );
}

function plural(count: number, one: string, many: string): string {
  return `${count} ${count > 1 ? many : one}`;
}
