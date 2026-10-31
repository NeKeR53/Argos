import { describe, expect, it } from "vitest";
import { admit, base64, MAX_ATTACHMENTS, pastedName, toAttachment } from "./attachments";

describe("pièces jointes", () => {
  it("encodent en base64, même au-delà d'une tranche", async () => {
    expect(base64(new TextEncoder().encode("Argos"))).toBe("QXJnb3M=");
    const big = new Uint8Array(0x8000 * 2 + 3).fill(65);
    expect(atob(base64(big)).length).toBe(big.length);
    const attachment = await toAttachment(new File(["# Titre"], "notes.md", { type: "text/markdown" }));
    expect(attachment).toEqual({ name: "notes.md", media_type: "text/markdown", data: btoa("# Titre") });
  });

  it("refusent au-delà des limites du daemon", () => {
    const small = (name: string) => new File(["x"], name);
    const many = Array.from({ length: MAX_ATTACHMENTS + 1 }, (_, i) => small(`f${i}`));
    const { files, refused } = admit([], many);
    expect(files).toHaveLength(MAX_ATTACHMENTS);
    expect(refused).toMatch(/au plus/);
    const huge = { name: "gros.bin", size: 11 * 1024 * 1024 } as File;
    expect(admit([], [huge]).refused).toBe("gros.bin dépasse 10 Mo");
  });

  it("nomment une image collée", () => {
    expect(pastedName(new File(["x"], "image.png", { type: "image/png" }), 0).name).toBe("capture-1.png");
    expect(pastedName(new File(["x"], "plan.jpg", { type: "image/jpeg" }), 0).name).toBe("plan.jpg");
  });
});
