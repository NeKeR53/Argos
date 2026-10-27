//! Fenêtre de l'orbe (D38) : une pastille discrète que l'on déplace, et qui grandit quand
//! il se passe quelque chose. La fenêtre suit la taille de la pastille, à centre fixe,
//! pour ne jamais masquer plus que nécessaire. Sa position (le centre, en points) est
//! gardée dans `$ARGOS_HOME/gui-state.json` et restaurée au lancement, ramenée dans un
//! écran si l'écran a changé.

use crate::daemon;
use crate::shell::ORB_WINDOW;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Runtime, WebviewWindow};

/// Taille de la fenêtre au repos, en points (pastille de 120×36 et sa marge).
const IDLE_SIZE: Size = Size {
    width: 128.0,
    height: 44.0,
};
/// Par défaut : centrée en bas de l'écran principal, au-dessus du Dock.
const DEFAULT_BOTTOM_MARGIN: f64 = 72.0;
/// Délai sans mouvement avant d'écrire la position (un glisser en produit des dizaines).
const SAVE_DELAY: Duration = Duration::from_millis(600);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

/// Rectangle en points (zone utile d'un écran : sans barre des menus ni Dock).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn contains(&self, point: Point) -> bool {
        point.x >= self.x
            && point.x <= self.x + self.width
            && point.y >= self.y
            && point.y <= self.y + self.height
    }
}

/// Fichier d'état de la GUI (`gui-state.json`) ; d'autres réglages d'interface pourront
/// s'y ajouter.
#[derive(Debug, Default, Serialize, Deserialize)]
struct GuiState {
    /// Centre de la fenêtre de l'orbe, en points.
    orb_center: Option<Point>,
}

/// Centre par défaut : en bas au milieu de la zone utile.
pub fn default_center(area: Rect) -> Point {
    Point {
        x: area.x + area.width / 2.0,
        y: area.y + area.height - DEFAULT_BOTTOM_MARGIN,
    }
}

/// Où poser l'orbe : le centre gardé s'il tombe dans un écran (ramené pour que la fenêtre
/// y tienne entière), sinon le centre par défaut du premier écran.
pub fn fit_center(saved: Option<Point>, size: Size, areas: &[Rect]) -> Option<Point> {
    let first = *areas.first()?;
    let (center, area) = match saved.and_then(|point| {
        areas
            .iter()
            .find(|area| area.contains(point))
            .map(|area| (point, *area))
    }) {
        Some(found) => found,
        None => (default_center(first), first),
    };
    Some(clamp(center, size, area))
}

/// Ramène un centre pour que la fenêtre de taille `size` tienne dans `area`.
fn clamp(center: Point, size: Size, area: Rect) -> Point {
    let half_w = size.width / 2.0;
    let half_h = size.height / 2.0;
    let min_x = area.x + half_w;
    let max_x = (area.x + area.width - half_w).max(min_x);
    let min_y = area.y + half_h;
    let max_y = (area.y + area.height - half_h).max(min_y);
    Point {
        x: center.x.clamp(min_x, max_x),
        y: center.y.clamp(min_y, max_y),
    }
}

/// Position et taille courantes de l'orbe, mémorisées pour garder le centre fixe quand la
/// taille change, et pour savoir quoi écrire.
#[derive(Default)]
pub struct OrbPlacement {
    center: Mutex<Option<Point>>,
    size: Mutex<Option<Size>>,
    /// Génération du dernier mouvement : seule la dernière écrit le fichier.
    moves: Arc<AtomicU64>,
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn state_path() -> Option<PathBuf> {
    daemon::argos_home()
        .ok()
        .map(|home| home.join("gui-state.json"))
}

fn read_state() -> GuiState {
    state_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_center(center: Option<Point>) {
    let Some(path) = state_path() else {
        return;
    };
    let mut state = read_state();
    state.orb_center = center;
    if let Ok(text) = serde_json::to_string_pretty(&state)
        && let Err(error) = std::fs::write(&path, text)
    {
        eprintln!("orbe : position non gardée ({}) : {error}", path.display());
    }
}

/// Zones utiles des écrans, en points.
fn work_areas<R: Runtime>(window: &WebviewWindow<R>) -> Vec<Rect> {
    let mut monitors = window.available_monitors().unwrap_or_default();
    // L'écran principal d'abord : c'est lui qui reçoit la position par défaut.
    if let Ok(Some(primary)) = window.primary_monitor()
        && let Some(index) = monitors.iter().position(|m| m.name() == primary.name())
    {
        monitors.swap(0, index);
    }
    monitors
        .iter()
        .map(|monitor| {
            let scale = monitor.scale_factor();
            let area = monitor.work_area();
            Rect {
                x: area.position.x as f64 / scale,
                y: area.position.y as f64 / scale,
                width: area.size.width as f64 / scale,
                height: area.size.height as f64 / scale,
            }
        })
        .collect()
}

fn apply<R: Runtime>(window: &WebviewWindow<R>, center: Point, size: Size) {
    let _ = window.set_size(LogicalSize::new(size.width, size.height));
    let _ = window.set_position(LogicalPosition::new(
        center.x - size.width / 2.0,
        center.y - size.height / 2.0,
    ));
}

/// Au lancement : taille de repos, position gardée (ou par défaut), dans un écran.
pub fn restore<R: Runtime>(app: &AppHandle<R>) {
    let (Some(window), Some(placement)) = (
        app.get_webview_window(ORB_WINDOW),
        app.try_state::<OrbPlacement>(),
    ) else {
        return;
    };
    let Some(center) = fit_center(read_state().orb_center, IDLE_SIZE, &work_areas(&window)) else {
        return;
    };
    *locked(&placement.center) = Some(center);
    *locked(&placement.size) = Some(IDLE_SIZE);
    apply(&window, center, IDLE_SIZE);
}

/// La pastille change de taille : la fenêtre suit, centre fixe, sans sortir de l'écran.
pub fn resize<R: Runtime>(app: &AppHandle<R>, width: f64, height: f64) {
    let (Some(window), Some(placement)) = (
        app.get_webview_window(ORB_WINDOW),
        app.try_state::<OrbPlacement>(),
    ) else {
        return;
    };
    let size = Size {
        width: width.clamp(40.0, 480.0),
        height: height.clamp(24.0, 160.0),
    };
    let saved = *locked(&placement.center);
    let center = fit_center(saved, size, &work_areas(&window)).unwrap_or(Point {
        x: size.width / 2.0,
        y: size.height / 2.0,
    });
    *locked(&placement.size) = Some(size);
    apply(&window, center, size);
}

/// Double-clic ou menu : retour à la place par défaut, et l'ancienne est oubliée.
pub fn reset<R: Runtime>(app: &AppHandle<R>) {
    let (Some(window), Some(placement)) = (
        app.get_webview_window(ORB_WINDOW),
        app.try_state::<OrbPlacement>(),
    ) else {
        return;
    };
    let size = locked(&placement.size).unwrap_or(IDLE_SIZE);
    let Some(center) = fit_center(None, size, &work_areas(&window)) else {
        return;
    };
    *locked(&placement.center) = Some(center);
    apply(&window, center, size);
    write_center(None);
}

/// La fenêtre a bougé (glisser, ou nos propres redimensionnements) : nouveau centre,
/// écrit après un court silence.
pub fn moved<R: Runtime>(app: &AppHandle<R>) {
    let (Some(window), Some(placement)) = (
        app.get_webview_window(ORB_WINDOW),
        app.try_state::<OrbPlacement>(),
    ) else {
        return;
    };
    let (Ok(position), Ok(size), Ok(scale)) = (
        window.outer_position(),
        window.outer_size(),
        window.scale_factor(),
    ) else {
        return;
    };
    let center = Point {
        x: (position.x as f64 + size.width as f64 / 2.0) / scale,
        y: (position.y as f64 + size.height as f64 / 2.0) / scale,
    };
    *locked(&placement.center) = Some(center);
    let generation = placement.moves.fetch_add(1, Ordering::SeqCst) + 1;
    let moves = Arc::clone(&placement.moves);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SAVE_DELAY).await;
        if moves.load(Ordering::SeqCst) == generation {
            write_center(Some(center));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP: Rect = Rect {
        x: 0.0,
        y: 25.0,
        width: 1512.0,
        height: 920.0,
    };
    const EXTERNAL: Rect = Rect {
        x: 1512.0,
        y: 0.0,
        width: 1920.0,
        height: 1055.0,
    };

    #[test]
    fn the_default_place_is_at_the_bottom_middle_of_the_first_screen() {
        let center = fit_center(None, IDLE_SIZE, &[LAPTOP, EXTERNAL]).unwrap();
        assert_eq!(
            center,
            Point {
                x: 756.0,
                y: 25.0 + 920.0 - DEFAULT_BOTTOM_MARGIN
            }
        );
    }

    #[test]
    fn a_saved_place_is_kept_on_any_screen() {
        let saved = Point {
            x: 2000.0,
            y: 300.0,
        };
        assert_eq!(
            fit_center(Some(saved), IDLE_SIZE, &[LAPTOP, EXTERNAL]),
            Some(saved)
        );
    }

    #[test]
    fn a_place_on_a_screen_that_is_gone_falls_back_to_the_default() {
        let on_external = Point {
            x: 2000.0,
            y: 300.0,
        };
        assert_eq!(
            fit_center(Some(on_external), IDLE_SIZE, &[LAPTOP]),
            fit_center(None, IDLE_SIZE, &[LAPTOP])
        );
        assert_eq!(fit_center(Some(on_external), IDLE_SIZE, &[]), None);
    }

    #[test]
    fn a_grown_window_stays_inside_its_screen() {
        // Collée au bord droit : en grandissant, elle se décale au lieu de déborder.
        let saved = Point {
            x: 1500.0,
            y: 940.0,
        };
        let wide = Size {
            width: 260.0,
            height: 60.0,
        };
        let center = fit_center(Some(saved), wide, &[LAPTOP]).unwrap();
        assert_eq!(center.x, 1512.0 - 130.0);
        assert_eq!(center.y, 25.0 + 920.0 - 30.0);
    }

    #[test]
    fn the_state_file_tolerates_missing_fields() {
        let state: GuiState = serde_json::from_str("{}").unwrap();
        assert_eq!(state.orb_center, None);
        let state: GuiState =
            serde_json::from_str(r#"{"orb_center":{"x":10.5,"y":20.0},"autre":1}"#).unwrap();
        assert_eq!(state.orb_center, Some(Point { x: 10.5, y: 20.0 }));
    }
}
