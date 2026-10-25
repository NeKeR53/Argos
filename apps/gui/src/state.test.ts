import { describe, expect, it } from "vitest";
import type { AgentStatus, ServerMessage } from "./protocol";
import { parseServerMessage } from "./protocol";
import {
  initialState,
  reduce,
  selectedAgent,
  treeOf,
  THREAD_LIMIT,
  type Action,
  type AppState,
} from "./state";

function agent(id: string, level: AgentStatus["level"], parent: string | null, extra: Partial<AgentStatus> = {}) {
  return {
    id,
    level,
    parent,
    engine: "claude",
    model: null,
    workdir: `/tmp/${id}`,
    charter: level === "agent" ? { name: id.toUpperCase(), scope: "essais" } : null,
    state: "parked",
    session_id: null,
    created_at: 1,
    running: false,
    busy: false,
    ...extra,
  } satisfies AgentStatus;
}

const assistant = agent("a0", "assistant", null);
const domotique = agent("dom", "agent", "a0");
const worker = agent("w1", "worker", "dom");

/** Passe par le parsing, comme une vraie trame du daemon. */
function server(message: ServerMessage): Action {
  return { type: "server", message: parseServerMessage(JSON.stringify(message)) };
}

function run(...actions: Action[]): AppState {
  return actions.reduce(reduce, initialState);
}

const connected: Action = { type: "connection", connection: { status: "connected" } };
const tree = server({ type: "reply", id: 1, reply: { kind: "tree", agents: [assistant, domotique, worker] } });

describe("arbre", () => {
  it("se construit depuis tree, chacun sous son parent", () => {
    const state = run(connected, tree);
    const roots = treeOf(state);
    expect(roots.map((node) => node.agent.id)).toEqual(["a0"]);
    expect(roots[0].children.map((node) => node.agent.id)).toEqual(["dom"]);
    expect(roots[0].children[0].children.map((node) => node.agent.id)).toEqual(["w1"]);
  });

  it("suit agent_updated : mise à jour, apparition, retrait", () => {
    const busy = { ...assistant, state: "active" as const, running: true, busy: true };
    const compta = agent("compta", "agent", "a0");
    let state = run(
      tree,
      server({ type: "event", event: { type: "agent_updated", agent: busy } }),
      server({ type: "event", event: { type: "agent_updated", agent: compta } }),
    );
    expect(state.agents.a0.busy).toBe(true);
    expect(state.order).toEqual(["a0", "dom", "w1", "compta"]);

    state = reduce(state, { type: "select", agent: "dom" });
    for (const retired of [worker, domotique]) {
      state = reduce(
        state,
        server({ type: "event", event: { type: "agent_updated", agent: { ...retired, state: "retired" } } }),
      );
    }
    expect(state.order).toEqual(["a0", "compta"]);
    // L'agent affiché a disparu : retour à l'Assistant.
    expect(selectedAgent(state)?.id).toBe("a0");
  });

  it("garde visible un agent dont le parent est inconnu", () => {
    const state = run(server({ type: "event", event: { type: "agent_updated", agent: worker } }));
    expect(treeOf(state).map((node) => node.agent.id)).toEqual(["w1"]);
  });

  it("affiche l'Assistant par défaut", () => {
    expect(selectedAgent(run(tree))?.id).toBe("a0");
    expect(selectedAgent(run(tree, { type: "select", agent: "w1" }))?.id).toBe("w1");
  });
});

describe("fil des messages", () => {
  it("assemble message utilisateur, réponse, outil et fin de tour", () => {
    const state = run(
      connected,
      tree,
      { type: "requested", id: 2, command: { command: "send", agent: "a0", text: "bonjour" } },
      server({ type: "reply", id: 2, reply: { kind: "done", affected: ["a0"] } }),
      server({ type: "event", event: { type: "tool", agent: "a0", name: "Bash" } }),
      server({ type: "event", event: { type: "message", agent: "a0", text: "reply 1" } }),
      server({
        type: "event",
        event: {
          type: "turn_completed",
          agent: "a0",
          outcome: { status: "completed" },
          usage: { input_tokens: 13, output_tokens: 2, cost_usd: 0.001 },
        },
      }),
      server({ type: "event", event: { type: "exited", agent: "a0", code: 0 } }),
    );
    const thread = state.threads.a0.map(({ seq: _, ...entry }) => entry);
    expect(thread).toEqual([
      { kind: "user", text: "bonjour", request: 2, delivery: "delivered" },
      { kind: "tool", name: "Bash" },
      { kind: "agent", text: "reply 1" },
      {
        kind: "turn",
        outcome: { status: "completed" },
        usage: { input_tokens: 13, output_tokens: 2, cost_usd: 0.001 },
      },
      { kind: "exited", code: 0 },
    ]);
    expect(state.spent.a0).toBeCloseTo(0.001);
    expect(state.pending).toEqual({});
  });

  it("cumule le coût des tours et ignore un coût absent", () => {
    const turn = (cost: number | null): Action =>
      server({
        type: "event",
        event: {
          type: "turn_completed",
          agent: "a0",
          outcome: { status: "failed", message: "quota" },
          usage: cost == null ? null : { input_tokens: 1, output_tokens: 1, cost_usd: cost },
        },
      });
    const state = run(turn(0.5), turn(null), turn(0.25));
    expect(state.spent.a0).toBeCloseTo(0.75);
  });

  it("borne la longueur d'un fil", () => {
    const message: Action = server({ type: "event", event: { type: "message", agent: "a0", text: "x" } });
    const state = run(...Array.from({ length: THREAD_LIMIT + 10 }, () => message));
    expect(state.threads.a0).toHaveLength(THREAD_LIMIT);
    expect(state.threads.a0[THREAD_LIMIT - 1].seq).toBe(THREAD_LIMIT + 10);
  });

  it("marque perdus les messages sans réponse quand la connexion tombe", () => {
    const state = run(
      connected,
      { type: "requested", id: 3, command: { command: "send", agent: "a0", text: "tu es là ?" } },
      { type: "connection", connection: { status: "reconnecting", attempt: 1, retryInMs: 500, reason: "connexion perdue" } },
    );
    expect(state.threads.a0[0]).toMatchObject({ kind: "user", delivery: "lost" });
    expect(state.pending).toEqual({});
  });
});

describe("erreurs du daemon", () => {
  it("deviennent des avis lisibles, avec la commande en contexte", () => {
    const state = run(
      tree,
      { type: "requested", id: 4, command: { command: "park", agent: "dom" } },
      server({ type: "reply", id: 4, reply: { kind: "error", message: "agent inconnu" } }),
    );
    expect(state.notices).toEqual([{ id: expect.any(Number), tone: "error", text: "Parking de DOM : agent inconnu" }]);
  });

  it("font échouer le message refusé", () => {
    const state = run(
      tree,
      { type: "requested", id: 5, command: { command: "send", agent: "w1", text: "salut" } },
      server({
        type: "reply",
        id: 5,
        reply: { kind: "error", message: "w1 est un Worker : l'utilisateur passe par son Agent" },
      }),
    );
    expect(state.threads.w1[0]).toMatchObject({ delivery: "failed" });
    expect(state.notices[0].text).toBe("Message à w1 : w1 est un Worker : l'utilisateur passe par son Agent");
  });

  it("restent lisibles pour une requête illisible (id 0)", () => {
    const state = run(server({ type: "reply", id: 0, reply: { kind: "error", message: "requête illisible : x" } }));
    expect(state.notices[0].text).toBe("Le daemon signale : requête illisible : x");
  });

  it("se ferment", () => {
    const state = run(server({ type: "reply", id: 0, reply: { kind: "error", message: "x" } }));
    expect(reduce(state, { type: "dismiss", notice: state.notices[0].id }).notices).toEqual([]);
  });

  it("préviennent d'une version de protocole différente", () => {
    expect(run(server({ type: "hello", protocol: 1 })).notices).toEqual([]);
    expect(run(server({ type: "hello", protocol: 2 })).notices[0].tone).toBe("warning");
  });
});
