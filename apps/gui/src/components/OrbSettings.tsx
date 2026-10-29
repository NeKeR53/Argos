import { useEffect, useRef, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type Placement = "floating" | "dock";

export function OrbSettings({ onClose }: { onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [placement, setPlacement] = useState<Placement | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const native = isTauri();

  useEffect(() => {
    const element = dialog.current;
    element?.showModal();
    let active = true;
    const updates = isTauri() ? listen<Placement>("orb-placement", (event) => {
      if (active) setPlacement(event.payload);
    }) : null;
    if (isTauri()) {
      void invoke<Placement>("orb_placement")
        .then((value) => { if (active) setPlacement(value); })
        .catch((reason: unknown) => { if (active) setError(String(reason)); });
    }
    return () => {
      active = false;
      element?.close();
      if (updates) void updates.then((stop) => stop());
    };
  }, []);

  const change = async (value: Placement) => {
    setSaving(true);
    setError(null);
    try {
      await invoke("set_orb_placement", { placement: value });
      setPlacement(value);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setSaving(false);
    }
  };

  return (
    <dialog ref={dialog} className="dialog" aria-labelledby="orb-settings-title"
      onCancel={(event) => { event.preventDefault(); if (!saving) onClose(); }}>
      <h2 id="orb-settings-title">Globe Argos</h2>
      <div className="dialog-body">
        <label htmlFor="orb-placement">Emplacement du globe</label>{" "}
        <select id="orb-placement" value={placement ?? ""} disabled={!native || placement === null || saving}
          onChange={(event) => void change(event.target.value as Placement)}>
          {placement === null && <option value="">{native ? "Chargement…" : "Application macOS requise"}</option>}
          <option value="floating">Globe flottant</option>
          <option value="dock">Globe animé dans le Dock</option>
        </select>
        <p className="muted">Dans le Dock, le globe suit l’activité d’Argos et remplace le globe flottant.
          Un clic ouvre la console. Les raccourcis vocaux restent actifs.</p>
        <p className="muted">Le choix est conservé au prochain lancement.</p>
        {error && <p role="alert" className="shortcut-error">{error}</p>}
      </div>
      <div className="dialog-actions">
        <button type="button" className="button button-primary" onClick={onClose} disabled={saving}>Fermer</button>
      </div>
    </dialog>
  );
}
