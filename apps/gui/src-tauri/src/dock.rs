//! L'orbe dans le Dock : la pieuvre animée en AppKit, indépendante de la webview masquée.
//! La lueur autour dit l'état ; la pieuvre (`octopus.rs`) le joue.

use crate::{orb::Placement, shell::ORB_WINDOW};
use serde::Deserialize;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Offline,
    Idle,
    Thinking,
    Listening,
    Speaking,
}

#[derive(Debug, Default, Clone, Copy, Deserialize)]
pub struct View {
    pub mode: Mode,
    pub level: f64,
    pub armed: bool,
    pub stopped: bool,
    pub escalation: bool,
    pub still: bool,
}

#[derive(Default)]
pub struct Dock {
    enabled: AtomicBool,
    view: Mutex<View>,
}

pub fn enabled<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    app.state::<Dock>().enabled.load(Ordering::Relaxed)
}

pub fn update(app: &AppHandle, mut view: View) {
    view.level = if view.level.is_finite() {
        view.level.clamp(0.0, 1.0)
    } else {
        0.0
    };
    *app.state::<Dock>()
        .view
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = view;
}

pub fn set_placement(app: &AppHandle, placement: Placement) -> Result<(), String> {
    let previous = if enabled(app) {
        Placement::Dock
    } else {
        Placement::Floating
    };
    crate::orb::save_placement(placement)?;
    if let Err(error) = apply(app, placement) {
        let _ = crate::orb::save_placement(previous);
        return Err(error);
    }
    let _ = app.emit("orb-placement", placement);
    Ok(())
}

fn apply(app: &AppHandle, placement: Placement) -> Result<(), String> {
    let in_dock = placement == Placement::Dock;
    #[cfg(not(target_os = "macos"))]
    if in_dock {
        return Err("le globe dans le Dock est réservé à macOS".into());
    }
    let window = app
        .get_webview_window(ORB_WINDOW)
        .ok_or("fenêtre de l'orbe absente")?;
    #[cfg(target_os = "macos")]
    {
        app.set_activation_policy(if in_dock {
            tauri::ActivationPolicy::Regular
        } else {
            tauri::ActivationPolicy::Accessory
        })
        .map_err(|error| error.to_string())?;
        native::clear()?;
    }
    // Masquée, la webview de l'orbe continue de mener la voix : `backgroundThrottling`
    // est désactivé pour elle dans `tauri.conf.json`, sinon WebKit suspend son JavaScript
    // et la lecture s'arrête après la première phrase.
    if in_dock {
        window.hide()
    } else {
        window.show()
    }
    .map_err(|error| error.to_string())?;
    app.state::<Dock>()
        .enabled
        .store(in_dock, Ordering::Relaxed);
    Ok(())
}

pub fn start(app: &AppHandle) {
    if let Err(error) = apply(app, crate::orb::placement()) {
        eprintln!("Dock : {error}");
    }
    #[cfg(target_os = "macos")]
    {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let start = std::time::Instant::now();
            // Une seule image en attente, même quand le thread principal est occupé.
            let pending = std::sync::Arc::new(AtomicBool::new(false));
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                if !enabled(&app) || pending.swap(true, Ordering::Relaxed) {
                    continue;
                }
                let handle = app.clone();
                let pending_frame = pending.clone();
                let time = start.elapsed().as_secs_f64();
                if app
                    .run_on_main_thread(move || {
                        if enabled(&handle) {
                            let view = *handle
                                .state::<Dock>()
                                .view
                                .lock()
                                .unwrap_or_else(|p| p.into_inner());
                            native::draw(view, time);
                        }
                        pending_frame.store(false, Ordering::Relaxed);
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::{Mode, View};
    use crate::octopus::{self, Pt};
    use objc2::rc::Retained;
    use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSBezierPath, NSColor, NSGradient, NSImage, NSImageView, NSLineCapStyle,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
    }

    pub fn clear() -> Result<(), String> {
        let mtm =
            MainThreadMarker::new().ok_or("le Dock doit être modifié sur le thread principal")?;
        let tile = NSApplication::sharedApplication(mtm).dockTile();
        tile.setContentView(None);
        tile.setBadgeLabel(None);
        tile.display();
        Ok(())
    }

    /// Coordonnées de la pieuvre (grille 64×64, origine en haut à gauche) vers la tuile
    /// de 128 points (origine en bas à gauche).
    fn at((x, y): Pt) -> NSPoint {
        NSPoint::new(x * 2.0, 128.0 - y * 2.0)
    }

    /// Ovale centré sur (cx, cy) de demi-axes (rx, ry), en coordonnées de la grille.
    fn oval(cx: f64, cy: f64, rx: f64, ry: f64) -> Retained<NSBezierPath> {
        NSBezierPath::bezierPathWithOvalInRect(rect(
            (cx - rx) * 2.0,
            128.0 - (cy + ry) * 2.0,
            rx * 4.0,
            ry * 4.0,
        ))
    }

    fn color(r: f64, g: f64, b: f64, a: f64) -> Retained<NSColor> {
        NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a)
    }

    /// Courbe quadratique en cubique (NSBezierPath n'a que les cubiques).
    fn quad(path: &NSBezierPath, from: Pt, control: Pt, to: Pt) {
        let c1 = (
            from.0 + 2.0 / 3.0 * (control.0 - from.0),
            from.1 + 2.0 / 3.0 * (control.1 - from.1),
        );
        let c2 = (
            to.0 + 2.0 / 3.0 * (control.0 - to.0),
            to.1 + 2.0 / 3.0 * (control.1 - to.1),
        );
        path.curveToPoint_controlPoint1_controlPoint2(at(to), at(c1), at(c2));
    }

    /// Contour fermé lissé : on passe par le milieu des côtés (même tracé que la console).
    fn smooth_path(points: &[Pt]) -> Retained<NSBezierPath> {
        let n = points.len();
        let mid = |a: Pt, b: Pt| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        let path = NSBezierPath::bezierPath();
        let mut from = mid(points[n - 1], points[0]);
        path.moveToPoint(at(from));
        for i in 0..n {
            let to = mid(points[i], points[(i + 1) % n]);
            quad(&path, from, points[i], to);
            from = to;
        }
        path.closePath();
        path
    }

    /// La pieuvre, d'après `octopus::pose` : la même que dans la console et l'orbe.
    fn draw_octopus(view: View, t: f64) {
        let mode = if view.armed || view.stopped {
            Mode::Idle
        } else {
            view.mode
        };
        let mouth = matches!(mode, Mode::Speaking).then(|| octopus::synthetic_viseme(t));
        let p = octopus::pose(mode, t, view.level, mouth);
        let offline = matches!(mode, Mode::Offline);
        // Hors ligne, la pieuvre pâlit.
        let fade = |c: (f64, f64, f64)| {
            if offline {
                let grey = 0.3 * c.0 + 0.59 * c.1 + 0.11 * c.2;
                (
                    lerp(c.0, grey, 0.6),
                    lerp(c.1, grey, 0.6),
                    lerp(c.2, grey, 0.6),
                )
            } else {
                c
            }
        };
        let rgb = |c: (f64, f64, f64), a: f64| {
            let (r, g, b) = fade(c);
            color(r, g, b, a)
        };
        let top = rgb((0.71, 0.66, 1.0), 1.0);
        let bottom = rgb((0.46, 0.39, 0.93), 1.0);
        let gradient =
            NSGradient::initWithStartingColor_endingColor(NSGradient::alloc(), &bottom, &top);
        let skin = |path: &NSBezierPath| match &gradient {
            Some(g) => g.drawInBezierPath_angle(path, 90.0),
            None => {
                bottom.setFill();
                path.fill();
            }
        };

        rgb((0.37, 0.31, 0.84), 1.0).setFill();
        for arm in &p.back {
            smooth_path(arm).fill();
        }
        for arm in &p.front {
            skin(&smooth_path(arm));
        }
        rgb((0.94, 0.92, 1.0), 0.75).setFill();
        for s in &p.suckers {
            oval(s.x, s.y, s.r, s.r).fill();
        }
        skin(&smooth_path(&p.head));
        // Reflet sur la tête.
        let (hx, hy) = p.head[42];
        color(1.0, 1.0, 1.0, 0.3).setFill();
        oval(hx + 3.0, hy + 5.5, 6.0, 3.0).fill();
        for arm in &p.over {
            skin(&smooth_path(arm));
        }

        for c in &p.cheeks {
            color(1.0, 0.56, 0.7, c.alpha).setFill();
            oval(c.x, c.y, c.r, c.r * 0.6).fill();
        }

        let ink = color(0.11, 0.10, 0.20, 1.0);
        for e in &p.eyes {
            if e.open < 0.18 {
                ink.setStroke();
                let lid = NSBezierPath::bezierPath();
                lid.moveToPoint(at((e.cx - e.rx * 0.85, e.cy)));
                quad(
                    &lid,
                    (e.cx - e.rx * 0.85, e.cy),
                    (e.cx, e.cy + 2.6),
                    (e.cx + e.rx * 0.85, e.cy),
                );
                lid.setLineWidth(3.0);
                lid.setLineCapStyle(NSLineCapStyle::Round);
                lid.stroke();
                continue;
            }
            // Paupière : l'œil s'écrase vers le bas ; la pupille suit, sans déborder.
            let shift = e.ry * (1.0 - e.open) * 0.4;
            color(0.99, 0.99, 1.0, 1.0).setFill();
            oval(e.cx, e.cy + shift, e.rx, e.ry * e.open).fill();
            ink.setFill();
            oval(e.px, e.py + shift, e.pr, e.pr * e.open).fill();
            NSColor::whiteColor().setFill();
            let hr = e.pr * 0.36;
            oval(
                e.px + e.pr * 0.38,
                e.py + shift - e.pr * 0.4 * e.open,
                hr,
                hr * e.open,
            )
            .fill();
        }

        if p.brow_alpha > 0.01 {
            color(0.23, 0.18, 0.48, p.brow_alpha).setStroke();
            for [a, c, b] in p.brows {
                let brow = NSBezierPath::bezierPath();
                brow.moveToPoint(at(a));
                quad(&brow, a, c, b);
                brow.setLineWidth(2.6);
                brow.setLineCapStyle(NSLineCapStyle::Round);
                brow.stroke();
            }
        }

        let m = &p.mouth;
        let lips = color(0.23, 0.12, 0.25, 1.0);
        let (l, r) = ((m.x - m.half, m.y), (m.x + m.half, m.y));
        let mouth_path = NSBezierPath::bezierPath();
        mouth_path.moveToPoint(at(l));
        if m.upper + m.lower < 0.9 {
            quad(&mouth_path, l, (m.x, m.y + m.curve), r);
            lips.setStroke();
            mouth_path.setLineWidth(3.0);
            mouth_path.setLineCapStyle(NSLineCapStyle::Round);
            mouth_path.stroke();
        } else {
            quad(&mouth_path, l, (m.x, m.y - m.upper), r);
            quad(&mouth_path, r, (m.x, m.y + m.lower), l);
            lips.setFill();
            mouth_path.fill();
            if m.lower > 3.0 {
                color(0.91, 0.42, 0.56, 1.0).setFill();
                oval(m.x, m.y + m.lower * 0.55, m.half * 0.45, m.lower * 0.2).fill();
            }
        }

        for b in &p.bubbles {
            if b.alpha > 0.02 {
                let bubble = oval(b.x, b.y, b.r, b.r);
                rgb((0.91, 0.89, 1.0), 0.35 * b.alpha).setFill();
                bubble.fill();
                rgb((0.71, 0.66, 1.0), b.alpha).setStroke();
                bubble.setLineWidth(1.2);
                bubble.stroke();
            }
        }
    }

    fn lerp(a: f64, b: f64, k: f64) -> f64 {
        a + (b - a) * k
    }

    // AppKit ne redessine pas automatiquement une tuile du Dock personnalisée.
    #[allow(deprecated)]
    pub fn draw(view: View, time: f64) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let tile = NSApplication::sharedApplication(mtm).dockTile();
        let t = if view.still { 0.0 } else { time };
        let pulse = (t * 2.2).sin() * 1.5;
        let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(128.0, 128.0));
        image.lockFocus();
        let (r, g, b) = if view.armed || view.stopped {
            (0.95, 0.28, 0.31)
        } else {
            match view.mode {
                Mode::Offline => (0.40, 0.43, 0.49),
                Mode::Thinking => (0.58, 0.48, 1.0),
                Mode::Listening => (0.37, 0.83, 0.69),
                Mode::Speaking => (0.50, 0.70, 1.0),
                Mode::Idle => (0.43, 0.76, 0.94),
            }
        };
        let glow = NSBezierPath::bezierPathWithOvalInRect(rect(
            5.0 - pulse,
            5.0 - pulse,
            118.0 + 2.0 * pulse,
            118.0 + 2.0 * pulse,
        ));
        NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 0.16).setFill();
        glow.fill();
        draw_octopus(view, t);
        image.unlockFocus();
        let content = NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, tile.size()),
        );
        content.setImage(Some(&image));
        tile.setContentView(Some(&content));
        let badge = if view.armed {
            Some("!!")
        } else if view.stopped {
            Some("×")
        } else if view.escalation {
            Some("!")
        } else {
            None
        };
        tile.setBadgeLabel(badge.map(NSString::from_str).as_deref());
        tile.display();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dock_view_rejects_unknown_modes() {
        assert!(serde_json::from_str::<View>(r#"{"mode":"unknown","level":0,"armed":false,"stopped":false,"escalation":false,"still":false}"#).is_err());
        let view: View = serde_json::from_str(r#"{"mode":"listening","level":0.8,"armed":false,"stopped":false,"escalation":true,"still":false}"#).unwrap();
        assert!(matches!(view.mode, Mode::Listening));
        assert_eq!(view.level, 0.8);
        assert!(view.escalation);
    }
}
