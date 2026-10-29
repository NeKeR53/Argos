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
    use objc2_app_kit::{NSApplication, NSBezierPath, NSColor, NSGradient, NSImage, NSImageView};
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
        let ball = NSBezierPath::bezierPathWithOvalInRect(rect(12.0, 12.0, 104.0, 104.0));
        let light = NSColor::colorWithSRGBRed_green_blue_alpha(r * 0.65, g * 0.65, b * 0.65, 1.0);
        let dark = NSColor::colorWithSRGBRed_green_blue_alpha(0.055, 0.065, 0.11, 1.0);
        if let Some(gradient) =
            NSGradient::initWithStartingColor_endingColor(NSGradient::alloc(), &light, &dark)
        {
            gradient.drawInBezierPath_angle(&ball, -65.0);
        }
        NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 0.85).setStroke();
        ball.setLineWidth(1.5);
        ball.stroke();
        NSColor::colorWithSRGBRed_green_blue_alpha(0.80, 0.95, 1.0, 1.0).setFill();
        if matches!(view.mode, Mode::Speaking) && !view.armed && !view.stopped {
            let blink = if !view.still && t % 4.2 > 4.05 {
                1.0
            } else {
                5.0
            };
            for x in [43.0, 77.0] {
                NSBezierPath::bezierPathWithOvalInRect(rect(x, 72.0, 7.0, blink)).fill();
            }
            let opening = 4.0 + 13.0 * (t * 8.0).sin().abs();
            NSBezierPath::bezierPathWithOvalInRect(rect(48.0, 43.0 - opening / 2.0, 32.0, opening))
                .fill();
        } else {
            for i in 0..9 {
                let x = 34.0 + i as f64 * 7.0;
                let wave = (t * 4.0 + i as f64 * 0.7).sin().abs();
                let amplitude = match view.mode {
                    Mode::Listening => 6.0 + 42.0 * view.level,
                    Mode::Thinking => 24.0,
                    Mode::Offline => 0.0,
                    _ => 9.0,
                };
                let height = 5.0 + amplitude * wave;
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    rect(x, 64.0 - height / 2.0, 4.0, height),
                    2.0,
                    2.0,
                )
                .fill();
            }
        }
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
