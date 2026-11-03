import { describe, expect, it } from "vitest";
import type { AgentStatus } from "../protocol";
import { machinesOf } from "./NodeMap";

function agent(id: string, level: AgentStatus["level"], parent: string | null, node: string | null = null): AgentStatus {
  return {
    id,
    level,
    parent,
    engine: "claude",
    model: null,
    profile: null,
    node,
    workdir: `/tmp/${id}`,
    charter: level === "agent" ? { name: id, scope: "essais" } : null,
    state: "parked",
    session_id: null,
    created_at: 1,
    running: false,
    busy: false,
  };
}

describe("machinesOf", () => {
  const agents = Object.fromEntries(
    [
      agent("zeta", "agent", "a0"),
      agent("a0", "assistant", null),
      agent("domotique", "agent", "a0", "hermes"),
      agent("w1", "worker", "domotique", "hermes"),
      agent("w2", "worker", "domotique", "hermes"),
      agent("vieux", "agent", "a0", "ancien"),
    ].map((a) => [a.id, a]),
  );

  it("place le hub d'abord, l'Assistant en tête, et compte les Workers sous leur Agent", () => {
    const [hub, ancien, hermes] = machinesOf(agents, [{ name: "hermes", connected: true, error: null }]);
    expect(hub.name).toBeNull();
    expect(hub.agents.map(({ agent }) => agent.id)).toEqual(["a0", "zeta"]);
    expect(hermes).toMatchObject({ name: "hermes", connected: true, declared: true });
    expect(hermes.agents).toEqual([{ agent: agents.domotique, workers: 2 }]);
    // Un nœud que plus rien ne déclare reste visible, injoignable.
    expect(ancien).toMatchObject({ name: "ancien", connected: false, declared: false });
  });

  it("montre un nœud déclaré même sans agent", () => {
    const machines = machinesOf({ a0: agents.a0 }, [{ name: "pi", connected: false, error: "refusé" }]);
    expect(machines.map((m) => [m.name, m.agents.length, m.error])).toEqual([
      [null, 1, null],
      ["pi", 0, "refusé"],
    ]);
  });
});
