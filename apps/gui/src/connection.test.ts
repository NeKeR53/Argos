import { describe, expect, it } from "vitest";
import { backoffDelay, endpointFromEnvironment } from "./connection";

describe("connexion", () => {
  it("double le délai à chaque échec, sans dépasser 10 s", () => {
    expect([1, 2, 3, 4, 5, 6, 12].map(backoffDelay)).toEqual([500, 1000, 2000, 4000, 8000, 10000, 10000]);
  });

  it("lit l'URL du daemon dans ?daemon= hors de Tauri", async () => {
    const url = "ws://127.0.0.1:7891/?token=abc";
    const endpoint = endpointFromEnvironment(`?daemon=${encodeURIComponent(url)}`);
    expect(await endpoint?.resolve()).toBe(url);
    expect(endpoint?.probe).toBeUndefined();
    expect(endpointFromEnvironment("")).toBeNull();
  });
});
