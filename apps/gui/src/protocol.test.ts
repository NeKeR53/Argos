// Les échantillons JSON sont ceux des tests de `crates/argos-core/src/protocol.rs` :
// s'ils changent là-bas, ils changent ici.

import { describe, expect, it } from "vitest";
import { encodeRequest, parseServerMessage, ProtocolError } from "./protocol";

describe("requêtes", () => {
  it("sont des objets plats", () => {
    expect(JSON.parse(encodeRequest(7, { command: "send", agent: "a1", text: "bonjour" }))).toEqual({
      id: 7,
      command: "send",
      agent: "a1",
      text: "bonjour",
    });
    expect(JSON.parse(encodeRequest(8, { command: "panic_stop" }))).toEqual({ id: 8, command: "panic_stop" });
  });
});

describe("messages du daemon", () => {
  it("sont étiquetés", () => {
    const reply = { type: "reply", id: 7, reply: { kind: "error", message: "agent inconnu" } };
    expect(parseServerMessage(JSON.stringify(reply))).toEqual(reply);

    const event = {
      type: "event",
      event: {
        type: "turn_completed",
        agent: "a1",
        outcome: { status: "failed", message: "quota" },
        usage: null,
      },
    };
    expect(parseServerMessage(JSON.stringify(event))).toEqual(event);
  });

  it("lisent un AgentStatus aplati", () => {
    const message = parseServerMessage(
      JSON.stringify({
        type: "reply",
        id: 1,
        reply: {
          kind: "tree",
          agents: [
            {
              id: "a0",
              level: "assistant",
              parent: null,
              engine: "claude",
              model: "claude-opus-5-5",
              workdir: "/tmp/a0",
              charter: null,
              state: "parked",
              session_id: null,
              created_at: 1792962715,
              running: false,
              busy: false,
            },
          ],
        },
      }),
    );
    expect(message.type === "reply" && message.reply.kind === "tree" && message.reply.agents[0].level).toBe(
      "assistant",
    );
  });

  it("acceptent hello", () => {
    expect(parseServerMessage('{"type":"hello","protocol":1}')).toEqual({ type: "hello", protocol: 1 });
  });

  it("rejettent ce qui sort du protocole", () => {
    for (const frame of [
      "pas du json",
      "[]",
      '{"type":"bonjour"}',
      '{"type":"reply","id":1,"reply":{"kind":"peut-etre"}}',
      '{"type":"event","event":{"type":"message","agent":"a1"}}',
      '{"type":"event","event":{"type":"turn_completed","agent":"a1","outcome":{"status":"bof"},"usage":null}}',
      '{"type":"event","event":{"type":"agent_updated","agent":{"id":"a1","level":"assistant"}}}',
    ]) {
      expect(() => parseServerMessage(frame), frame).toThrow(ProtocolError);
    }
  });
});
