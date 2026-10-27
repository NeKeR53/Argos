// Arbre des agents : chacun sous son parent (Assistant → Agents → Workers).

import type { AgentId, AgentStatus } from "../protocol";
import { displayName, shortId, type TreeNode } from "../state";
import { engineLabel, levelLabel, stateLabel } from "../format";
import { StatusDot } from "./StatusDot";

interface Props {
  roots: TreeNode[];
  selected: AgentId | null;
  /** Escalades non lues, par agent qui les a reçues (l'Assistant). */
  alerts: Record<AgentId, number>;
  onSelect: (agent: AgentId) => void;
}

export function AgentTree({ roots, ...rest }: Props) {
  return (
    <ul className="tree" role="tree" aria-label="Arbre des agents">
      {roots.map((node) => (
        <Branch key={node.agent.id} node={node} depth={0} {...rest} />
      ))}
    </ul>
  );
}

type BranchProps = Omit<Props, "roots"> & { node: TreeNode; depth: number };

function Branch({ node, depth, selected, alerts, onSelect }: BranchProps) {
  const { agent, children } = node;
  const isSelected = agent.id === selected;
  const alert = alerts[agent.id] ?? 0;
  return (
    <li role="treeitem" aria-selected={isSelected} aria-expanded={children.length ? true : undefined}>
      <button
        type="button"
        className="tree-row"
        data-level={agent.level}
        data-selected={isSelected || undefined}
        style={{ paddingLeft: `${10 + depth * 16}px` }}
        onClick={() => onSelect(agent.id)}
        title={`${levelLabel[agent.level]} ${agent.id}`}
      >
        <StatusDot agent={agent} />
        <span className="tree-text">
          <span className="tree-name">
            {displayName(agent)}
            {agent.level !== "worker" && <span className="tree-id">{shortId(agent.id)}</span>}
            {alert > 0 && (
              <span className="alert-badge" title={`${alert} escalade(s) non lue(s)`}>
                {alert}
              </span>
            )}
          </span>
          <span className="tree-meta">
            {levelLabel[agent.level]} · {engineLabel(agent)}
            {agent.node && <span title={`Nœud ${agent.node}`}> · @{agent.node}</span>}
          </span>
        </span>
        <AgentFlags agent={agent} />
      </button>
      {children.length > 0 && (
        <ul role="group">
          {children.map((child) => (
            <Branch
              key={child.agent.id}
              node={child}
              depth={depth + 1}
              selected={selected}
              alerts={alerts}
              onSelect={onSelect}
            />
          ))}
        </ul>
      )}
    </li>
  );
}

/** État du registre (`active` / `parked`) et tour en cours, en étiquettes courtes. */
export function AgentFlags({ agent }: { agent: AgentStatus }) {
  return (
    <span className="flags">
      {agent.busy && (
        <span className="flag flag-busy" title="Un tour est en cours (busy)">
          tour
        </span>
      )}
      <span className="flag" data-state={agent.state} title={`État du registre : ${agent.state}`}>
        {stateLabel[agent.state]}
      </span>
    </span>
  );
}
