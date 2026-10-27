import { afterEach, describe, expect, it, vi } from "vitest";
import { DaemonConnection } from "./connection";

class Socket {
  static readonly OPEN = 1;
  static instances: Socket[] = [];
  readyState = 0;
  sent: { id: number; command: string }[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;

  constructor() { Socket.instances.push(this); }
  send(frame: string) { this.sent.push(JSON.parse(frame)); }
  open() { this.readyState = Socket.OPEN; this.onopen?.(); }
  close() { this.readyState = 3; this.onclose?.(); }
  reply(id: number, reply: unknown) { this.onmessage?.({ data: JSON.stringify({ type: "reply", id, reply }) }); }
}

async function connected() {
  vi.useFakeTimers();
  Socket.instances = [];
  vi.stubGlobal("WebSocket", Socket);
  const onMessage = vi.fn();
  const connection = new DaemonConnection({ resolve: async () => "ws://local" }, {
    onMessage, onConnection: vi.fn(), onRequest: vi.fn(),
  });
  connection.start();
  await Promise.resolve();
  const socket = Socket.instances[0];
  socket.open();
  return { connection, socket, onMessage };
}

const command = { command: "surface_call", surface: "s-1", function: "set_light", arguments: { state: true } } as const;

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("requêtes attendues du daemon", () => {
  it("corrèle les réponses et remet les erreurs à l'appelant", async () => {
    const { connection, socket } = await connected();
    const first = connection.request(command);
    const second = connection.request(command);
    socket.reply(3, { kind: "function_result", result: false });
    socket.reply(2, { kind: "function_result", result: { state: true } });
    expect(await second).toEqual({ kind: "function_result", result: false });
    expect(await first).toEqual({ kind: "function_result", result: { state: true } });
    const failed = connection.request(command);
    socket.reply(4, { kind: "error", message: "fonction absente" });
    await expect(failed).rejects.toThrow("fonction absente");
    connection.stop();
  });

  it("rejette les requêtes sur déconnexion sans rejouer une action", async () => {
    const { connection, socket } = await connected();
    const pending = connection.request(command);
    socket.close();
    await expect(pending).rejects.toThrow("résultat de l'action incertain");
    await expect(connection.request(command)).rejects.toThrow("hors connexion");
    expect(socket.sent.filter((frame) => frame.command === "surface_call")).toHaveLength(1);
    connection.stop();
  });

  it("expire les appels et ferme les attentes au démontage", async () => {
    const { connection, socket, onMessage } = await connected();
    const timedOut = connection.request(command).catch((error: Error) => error.message);
    await vi.advanceTimersByTimeAsync(35_000);
    expect(await timedOut).toContain("Délai dépassé");
    expect(onMessage).toHaveBeenCalledWith({
      type: "reply", id: 2, reply: { kind: "error", message: "Délai dépassé ; résultat de l'action incertain." },
    });
    const stopped = connection.request(command);
    connection.stop();
    await expect(stopped).rejects.toThrow("Connexion fermée");
    expect(socket.sent.filter((frame) => frame.command === "surface_call")).toHaveLength(2);
    expect(vi.getTimerCount()).toBe(0);
  });
});
