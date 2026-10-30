//! L'orbe dans le Dock : animation AppKit indépendante de la webview masquée.

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
                tokio::time::sleep(std::time::Duration::from_millis(80)).await;
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
    use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSBezierPath, NSColor, NSImage, NSImageView};
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
    /// de 128 points (origine en bas à gauche) : même dessin que la console.
    fn at(x: f64, y: f64) -> NSPoint {
        NSPoint::new(x * 2.0, 128.0 - y * 2.0)
    }

    /// Ovale centré sur (cx, cy) de demi-axes (rx, ry), en coordonnées de la grille.
    fn oval(cx: f64, cy: f64, rx: f64, ry: f64) -> NSRect {
        rect((cx - rx) * 2.0, 128.0 - (cy + ry) * 2.0, rx * 4.0, ry * 4.0)
    }

    /// La pieuvre : tête, tentacules, yeux qui clignent, bouche qui articule quand elle
    /// parle. La couleur de la lueur (et la pastille) disent toujours l'état.
    fn draw_octopus(view: View, t: f64, r: f64, g: f64, b: f64) {
        let speaking = matches!(view.mode, Mode::Speaking) && !view.armed && !view.stopped;
        let skin = NSColor::colorWithSRGBRed_green_blue_alpha(0.56, 0.47, 0.95, 1.0);
        let ink = NSColor::colorWithSRGBRed_green_blue_alpha(0.11, 0.10, 0.20, 1.0);
        let tentacles = [
            ([19.0, 40.0], [10.0, 46.0], [11.0, 54.0], [15.0, 57.0]),
            ([25.0, 44.0], [22.0, 50.0], [23.0, 55.0], [26.0, 58.0]),
            ([32.0, 45.0], [32.0, 50.0], [31.0, 54.0], [33.0, 58.0]),
            ([39.0, 44.0], [42.0, 50.0], [41.0, 55.0], [38.0, 58.0]),
            ([45.0, 40.0], [54.0, 46.0], [53.0, 54.0], [49.0, 57.0]),
        ];
        skin.setStroke();
        skin.setFill();
        for (start, c1, c2, end) in tentacles {
            let path = NSBezierPath::bezierPath();
            path.moveToPoint(at(start[0], start[1]));
            path.curveToPoint_controlPoint1_controlPoint2(
                at(end[0], end[1]),
                at(c1[0], c1[1]),
                at(c2[0], c2[1]),
            );
            path.setLineWidth(9.0);
            path.stroke();
            NSBezierPath::bezierPathWithOvalInRect(rect(
                at(end[0], end[1]).x - 4.5,
                at(end[0], end[1]).y - 4.5,
                9.0,
                9.0,
            ))
            .fill();
        }
        NSBezierPath::bezierPathWithOvalInRect(oval(32.0, 25.5, 22.0, 19.5)).fill();

        // Yeux : ils clignent deux fois par période de 4,2 s, sauf figés (`still`).
        let blink = !view.still && t % 4.2 > 4.05;
        let eye_ry = if blink { 0.6 } else { 3.8 };
        ink.setFill();
        for cx in [25.0, 39.0] {
            NSBezierPath::bezierPathWithOvalInRect(oval(cx, 28.0, 3.4, eye_ry)).fill();
        }
        if !blink {
            NSColor::whiteColor().setFill();
            for cx in [26.2, 40.2] {
                NSBezierPath::bezierPathWithOvalInRect(oval(cx, 26.6, 1.2, 1.2)).fill();
            }
        }

        // Bouche : une lèvre ouverte quand elle parle, un petit sourire sinon.
        ink.setFill();
        ink.setStroke();
        let mouth = NSBezierPath::bezierPath();
        if speaking {
            let open = 2.0 + 8.0 * (t * 8.0).sin().abs();
            mouth.moveToPoint(at(27.0, 36.5));
            mouth.curveToPoint_controlPoint1_controlPoint2(
                at(37.0, 36.5),
                at(30.0, 36.5 - open),
                at(34.0, 36.5 - open),
            );
            mouth.curveToPoint_controlPoint1_controlPoint2(
                at(27.0, 36.5),
                at(34.0, 36.5 + open),
                at(30.0, 36.5 + open),
            );
            mouth.fill();
        } else {
            mouth.moveToPoint(at(28.4, 36.1));
            mouth.curveToPoint_controlPoint1_controlPoint2(
                at(35.6, 36.1),
                at(30.5, 39.0),
                at(33.5, 39.0),
            );
            mouth.setLineWidth(3.0);
            mouth.stroke();
        }

        // Contour de la tête à la couleur de l'état.
        NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 0.85).setStroke();
        let outline = NSBezierPath::bezierPathWithOvalInRect(oval(32.0, 25.5, 22.0, 19.5));
        outline.setLineWidth(1.5);
        outline.stroke();
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
        draw_octopus(view, t, r, g, b);
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
