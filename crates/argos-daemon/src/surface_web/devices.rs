//! Appareils associés aux surfaces. Chacun reçoit son propre secret, posé en cookie, et se
//! révoque seul depuis l'index de la machine.
//!
//! On associe un appareil avec la clé d'association (`$ARGOS_HOME/surfaces-token`), que
//! porte le QR code de l'index, ou avec le code court qui en dérive, à taper sur
//! l'appareil (sans appareil photo, ou dans une app installée sur l'écran d'accueil, qui
//! n'a pas les cookies du navigateur). Renouveler la clé change le QR code et le code ; les
//! appareils déjà associés le restent.
//!
//! Le registre (`$ARGOS_HOME/surfaces-devices.json`) ne garde que l'empreinte des secrets.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Essais de code manqués tolérés par fenêtre, tous appareils confondus.
const MAX_FAILURES: usize = 10;
const FAILURE_WINDOW: Duration = Duration::from_secs(600);
/// Un passage n'est réécrit sur disque qu'au-delà de cet écart.
const SEEN_PRECISION: i64 = 3600;
/// Alphabet du code court, sans caractères qui se confondent (0/O, 1/I).
const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    /// Déduit du navigateur : « iPhone · Safari ».
    pub name: String,
    /// Empreinte BLAKE3 du secret.
    hash: String,
    pub paired_at: i64,
    pub last_seen: i64,
}

#[derive(Clone)]
pub struct Devices {
    inner: Arc<Mutex<State>>,
}

struct State {
    key: String,
    key_file: PathBuf,
    file: PathBuf,
    devices: Vec<Device>,
    failures: VecDeque<Instant>,
}

impl Devices {
    /// La clé d'association (créée au besoin) et le registre des appareils de `home`.
    pub fn open(home: &Path) -> std::io::Result<Self> {
        let key_file = home.join("surfaces-token");
        let key = crate::config::load_or_create_token(&key_file)?;
        let file = home.join("surfaces-devices.json");
        let devices = match std::fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str(&text).map_err(std::io::Error::other)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(error) => return Err(error),
        };
        Ok(Self {
            inner: Arc::new(Mutex::new(State {
                key,
                key_file,
                file,
                devices,
                failures: VecDeque::new(),
            })),
        })
    }

    pub fn key(&self) -> String {
        locked(&self.inner).key.clone()
    }

    /// Code court de la clé, à taper sur l'appareil : `ABCD-EFGH`.
    pub fn code(&self) -> String {
        short_code(&locked(&self.inner).key)
    }

    pub fn is_key(&self, candidate: &str) -> bool {
        same(&locked(&self.inner).key, candidate)
    }

    /// Un code tapé sur un appareil. Au-delà de dix échecs en dix minutes, plus aucun essai
    /// n'est accepté jusqu'à la fin de la fenêtre.
    pub fn try_code(&self, candidate: &str) -> Result<(), &'static str> {
        let mut state = locked(&self.inner);
        let now = Instant::now();
        while state
            .failures
            .front()
            .is_some_and(|at| now.duration_since(*at) > FAILURE_WINDOW)
        {
            state.failures.pop_front();
        }
        if state.failures.len() >= MAX_FAILURES {
            return Err("Trop d'essais : réessaie dans dix minutes.");
        }
        let typed: String = candidate
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_uppercase())
            .collect();
        let expected: String = short_code(&state.key).replace('-', "");
        if same(&expected, &typed) {
            return Ok(());
        }
        state.failures.push_back(now);
        Err("Code incorrect.")
    }

    /// Associe un nouvel appareil ; rend le secret à poser en cookie.
    pub fn pair(&self, user_agent: &str) -> std::io::Result<String> {
        let secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let now = unix_now();
        let mut state = locked(&self.inner);
        state.devices.push(Device {
            id: format!("d-{}", &Uuid::new_v4().simple().to_string()[..8]),
            name: device_name(user_agent),
            hash: blake3::hash(secret.as_bytes()).to_hex().to_string(),
            paired_at: now,
            last_seen: now,
        });
        state.save()?;
        Ok(secret)
    }

    /// L'appareil de ce secret, s'il est associé ; son passage est noté.
    pub fn recognize(&self, secret: &str) -> bool {
        let hash = blake3::hash(secret.as_bytes());
        let now = unix_now();
        let mut state = locked(&self.inner);
        let Some(device) = state
            .devices
            .iter_mut()
            .find(|device| blake3::Hash::from_hex(&device.hash).is_ok_and(|known| known == hash))
        else {
            return false;
        };
        let stale = now - device.last_seen > SEEN_PRECISION;
        device.last_seen = now;
        if stale && let Err(error) = state.save() {
            eprintln!("surfaces : registre des appareils non écrit : {error}");
        }
        true
    }

    pub fn list(&self) -> Vec<Device> {
        locked(&self.inner).devices.clone()
    }

    /// Révoque un appareil ; `false` s'il n'existe pas.
    pub fn revoke(&self, id: &str) -> std::io::Result<bool> {
        let mut state = locked(&self.inner);
        let before = state.devices.len();
        state.devices.retain(|device| device.id != id);
        if state.devices.len() == before {
            return Ok(false);
        }
        state.save()?;
        Ok(true)
    }

    /// Nouvelle clé d'association : l'ancien QR code et l'ancien code ne servent plus.
    pub fn renew(&self) -> std::io::Result<()> {
        let mut state = locked(&self.inner);
        match std::fs::remove_file(&state.key_file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        state.key = crate::config::load_or_create_token(&state.key_file)?;
        state.failures.clear();
        Ok(())
    }
}

impl State {
    fn save(&self) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(&self.devices).map_err(std::io::Error::other)?;
        let partial = self.file.with_extension("json.partial");
        std::fs::write(&partial, text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(partial, &self.file)
    }
}

/// Huit caractères tirés de la clé, en deux groupes.
fn short_code(key: &str) -> String {
    let digest = blake3::hash(key.as_bytes());
    let chars: String = digest.as_bytes()[..8]
        .iter()
        .map(|byte| ALPHABET[usize::from(byte % 32)] as char)
        .collect();
    format!("{}-{}", &chars[..4], &chars[4..])
}

/// Comparaison en temps constant, par empreinte.
fn same(expected: &str, candidate: &str) -> bool {
    blake3::hash(expected.as_bytes()) == blake3::hash(candidate.as_bytes())
}

/// « iPhone · Safari », d'après l'en-tête `User-Agent`.
fn device_name(user_agent: &str) -> String {
    let has = |needle: &str| user_agent.contains(needle);
    let system = if has("iPhone") {
        "iPhone"
    } else if has("iPad") {
        "iPad"
    } else if has("Android") {
        "Android"
    } else if has("Macintosh") {
        "Mac"
    } else if has("Windows") {
        "Windows"
    } else if has("Linux") {
        "Linux"
    } else {
        "Appareil"
    };
    let browser = if has("Edg/") {
        Some("Edge")
    } else if has("Firefox/") || has("FxiOS") {
        Some("Firefox")
    } else if has("Chrome/") || has("CriOS") {
        Some("Chrome")
    } else if has("Safari/") {
        Some("Safari")
    } else {
        None
    };
    match browser {
        Some(browser) => format!("{system} · {browser}"),
        None => system.into(),
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_device_has_its_own_secret_and_is_revoked_alone() {
        let home = tempfile::tempdir().unwrap();
        let devices = Devices::open(home.path()).unwrap();
        let phone = devices
            .pair("Mozilla/5.0 (iPhone; CPU iPhone OS 26_0) Safari/604.1")
            .unwrap();
        let laptop = devices
            .pair("Mozilla/5.0 (Windows NT 10.0) Chrome/140 Safari/537")
            .unwrap();
        assert!(devices.recognize(&phone) && devices.recognize(&laptop));
        assert!(!devices.recognize("inconnu"));
        let names: Vec<String> = devices.list().into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["iPhone · Safari", "Windows · Chrome"]);

        // Le registre ne garde pas les secrets, et survit à un redémarrage.
        let saved = std::fs::read_to_string(home.path().join("surfaces-devices.json")).unwrap();
        assert!(!saved.contains(&phone));
        let devices = Devices::open(home.path()).unwrap();
        let id = devices.list()[0].id.clone();
        assert!(devices.revoke(&id).unwrap());
        assert!(!devices.recognize(&phone) && devices.recognize(&laptop));
        assert!(!devices.revoke(&id).unwrap());
    }

    #[test]
    fn the_short_code_follows_the_key_and_resists_guessing() {
        let home = tempfile::tempdir().unwrap();
        let devices = Devices::open(home.path()).unwrap();
        let code = devices.code();
        assert_eq!(code.len(), 9);
        assert!(
            devices
                .try_code(&code.to_lowercase().replace('-', " "))
                .is_ok()
        );

        let old = devices.key();
        devices.renew().unwrap();
        assert!(!devices.is_key(&old) && devices.is_key(&devices.key()));
        assert_ne!(devices.code(), code);
        assert_eq!(devices.try_code(&code), Err("Code incorrect."));

        // Le premier échec est l'ancien code.
        for _ in 1..MAX_FAILURES {
            assert_eq!(devices.try_code("ZZZZ-ZZZZ"), Err("Code incorrect."));
        }
        // Bloqué, même avec le bon code.
        assert!(devices.try_code(&devices.code()).is_err());
    }
}
