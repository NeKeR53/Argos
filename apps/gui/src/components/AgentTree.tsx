// Arbre des agents : chacun sous son parent (Assistant → Agents → Workers). Le nom
// seulement, une pastille quand l'agent travaille et une pastille d'escalade ; le reste
// (moteur, modèle, état) est dans les réglages de l'agent.

import type { AgentId } from "../protocol";
import { displayName, type TreeNode } from "../state";
import { levelLabel } from "../format";

interface Props {
  roots: TreeNode[];
  selected: AgentId | null;
  /** Escalades non vues, par agent (voir `attention`). */
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
        style={{ paddingLeft: `${8 + depth * 16}px` }}
        onClick={() => onSelect(agent.id)}
        title={`${levelLabel[agent.level]} ${agent.id}`}
      >
        <span className="tree-name">{displayName(agent)}</span>
        {agent.busy && <span className="tree-working" role="img" aria-label="au travail" title="Au travail" />}
        {alert > 0 && (
          <span
            className="alert-badge"
            role="img"
            aria-label={`${alert} escalade${alert > 1 ? "s" : ""} non vue${alert > 1 ? "s" : ""}`}
            title={`${alert} escalade${alert > 1 ? "s" : ""} : cet agent a besoin de vous`}
          >
            {alert > 1 ? alert : ""}
          </span>
        )}
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
