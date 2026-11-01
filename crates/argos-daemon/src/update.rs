//! Mises à jour d'Argos (D46). Cherche une version plus récente parmi les releases
//! GitHub, télécharge l'archive de la plateforme et vérifie sa somme SHA-256. Remplace
//! ensuite l'app, les fichiers voisins et le daemon, fait migrer la configuration par le
//! nouveau binaire, puis relance le service. Solution maison plutôt que
//! `tauri-plugin-updater` : celui-ci ne remplace que l'app et exige des paquets signés
//! (voir D46).
//!
//! L'installation tourne dans un process à part (`argos-daemon update apply`), détaché
//! du daemon qu'elle remplace et relance. Elle écrit son avancement dans
//! `$ARGOS_HOME/updates/status.json`, que le daemon relancé sert à la console.

use argos_core::protocol::{ReleaseInfo, UpdateProgress, UpdateStatus, UpdateStep};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Releases du dépôt, les plus récentes d'abord. `ARGOS_UPDATE_URL` la remplace (essais).
pub const RELEASES_URL: &str = "https://api.github.com/repos/NeKeR53/Argos/releases?per_page=20";
/// Fichier écrit par l'installeur à côté du daemon : où sont l'app et le service.
pub const MANIFEST: &str = "install.toml";
/// Une recherche réussie resservie pendant ce temps (l'API GitHub limite les appels).
const CHECK_TTL: Duration = Duration::from_secs(3600);
/// Une installation qui n'a plus donné signe de vie depuis ce délai est tenue pour morte.
const STALE: Duration = Duration::from_secs(15 * 60);

pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("version du paquet en semver")
}

/// Archive de la plateforme et fichier de ses sommes, tels que les publie `release.yml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    pub archive: &'static str,
    pub sums: &'static str,
    /// Nom de l'app dans l'archive.
    pub app: &'static str,
    pub daemon: &'static str,
}

pub fn platform() -> Option<Platform> {
    platform_for(std::env::consts::OS, std::env::consts::ARCH)
}

pub fn platform_for(os: &str, arch: &str) -> Option<Platform> {
    let (archive, sums, app, daemon) = match (os, arch) {
        ("macos", "aarch64") => (
            "Argos-macOS-arm64.zip",
            "SHA256SUMS-macos-arm64.txt",
            "Argos.app",
            "argos-daemon",
        ),
        ("macos", "x86_64") => (
            "Argos-macOS-x86_64.zip",
            "SHA256SUMS-macos-x86_64.txt",
            "Argos.app",
            "argos-daemon",
        ),
        ("linux", "x86_64") => (
            "Argos-Linux-x86_64.tar.gz",
            "SHA256SUMS-linux-x86_64.txt",
            "Argos.AppImage",
            "argos-daemon",
        ),
        ("windows", "x86_64") => (
            "Argos-Windows-x64.zip",
            "SHA256SUMS-windows.txt",
            "Argos.exe",
            "argos-daemon.exe",
        ),
        _ => return None,
    };
    Some(Platform {
        archive,
        sums,
        app,
        daemon,
    })
}

/// Une release telle que la décrit l'API GitHub.
#[derive(Debug, Clone, Deserialize)]
pub struct GithubRelease {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<GithubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GithubAsset {
    pub name: String,
    pub browser_download_url: String,
}

/// Une version plus récente, installable sur cette plateforme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub version: Version,
    pub notes: String,
    pub page: String,
    pub archive_url: String,
    pub sums_url: String,
}

impl Candidate {
    pub fn info(&self) -> ReleaseInfo {
        ReleaseInfo {
            version: self.version.to_string(),
            notes: self.notes.clone(),
            url: self.page.clone(),
        }
    }
}

/// La plus haute version publiée au-dessus de `current` qui a les deux fichiers de la
/// plateforme. Les préversions comptent : toutes les versions 0.x en sont.
pub fn newest(
    releases: &[GithubRelease],
    current: &Version,
    platform: Platform,
) -> Option<Candidate> {
    releases
        .iter()
        .filter(|release| !release.draft)
        .filter_map(|release| {
            let version = Version::parse(release.tag_name.trim_start_matches('v')).ok()?;
            let url = |name: &str| {
                release
                    .assets
                    .iter()
                    .find(|asset| asset.name == name)
                    .map(|asset| asset.browser_download_url.clone())
            };
            Some(Candidate {
                version,
                notes: release.body.clone().unwrap_or_default(),
                page: release.html_url.clone(),
                archive_url: url(platform.archive)?,
                sums_url: url(platform.sums)?,
            })
        })
        .filter(|candidate| candidate.version > *current)
        .max_by(|a, b| a.version.cmp(&b.version))
}

/// La somme annoncée pour `name` dans un fichier `SHA256SUMS` (`<hex>  <nom>`).
pub fn expected_sum(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let hash = fields.next()?;
        let file = fields.next()?.trim_start_matches('*');
        (file == name && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut hasher = Sha256::new();
    io::copy(&mut fs::File::open(path)?, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("argos-daemon/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| error.to_string())
}

fn releases_url() -> String {
    std::env::var("ARGOS_UPDATE_URL").unwrap_or_else(|_| RELEASES_URL.into())
}

/// Interroge les releases et rend la version à installer, s'il y en a une.
pub async fn find() -> Result<Option<Candidate>, String> {
    let platform = platform().ok_or("pas d'archive publiée pour cette plateforme")?;
    let response = client()?
        .get(releases_url())
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|error| format!("releases illisibles : {error}"))?;
    let releases: Vec<GithubRelease> = response
        .json()
        .await
        .map_err(|error| format!("releases illisibles : {error}"))?;
    Ok(newest(&releases, &current_version(), platform))
}

async fn download(url: &str, path: &Path) -> Result<(), String> {
    let failed = |error: &dyn std::fmt::Display| format!("téléchargement de {url} : {error}");
    let mut response = client()?
        .get(url)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|error| failed(&error))?;
    let mut file = fs::File::create(path).map_err(|error| failed(&error))?;
    while let Some(chunk) = response.chunk().await.map_err(|error| failed(&error))? {
        file.write_all(&chunk).map_err(|error| failed(&error))?;
    }
    file.sync_all().map_err(|error| failed(&error))
}

/// Comment relancer le daemon une fois remplacé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    /// Agent launchd de la session (macOS).
    Launchd,
    /// Unité systemd « utilisateur » (Linux).
    Systemd,
    /// Lancé à la main ou par un lanceur : l'installation l'arrête et le relance.
    None,
}

/// Où Argos est installé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub daemon: PathBuf,
    pub app: Option<PathBuf>,
    pub service: Service,
    /// Libellé launchd ou nom de l'unité systemd.
    pub label: String,
}

#[derive(Debug, Default, Deserialize)]
struct Manifest {
    app: Option<PathBuf>,
    service: Option<Service>,
    label: Option<String>,
}

impl Layout {
    /// L'installation du daemon en cours d'exécution.
    pub fn detect() -> io::Result<Self> {
        let daemon = std::env::current_exe()?.canonicalize()?;
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_default();
        Self::for_daemon(daemon, &home, platform())
    }

    /// Lit `install.toml` à côté du daemon ; à défaut, devine d'après les emplacements
    /// habituels : l'app dans le même dossier (archive décompressée), sinon dans
    /// `/Applications` ou `~/Applications` ; le service s'il est déclaré au système.
    pub fn for_daemon(
        daemon: PathBuf,
        user_home: &Path,
        platform: Option<Platform>,
    ) -> io::Result<Self> {
        let dir = daemon.parent().unwrap_or(Path::new(".")).to_path_buf();
        let manifest: Manifest = match fs::read_to_string(dir.join(MANIFEST)) {
            Ok(text) => toml::from_str(&text).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{MANIFEST} illisible : {error}"),
                )
            })?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Manifest::default(),
            Err(error) => return Err(error),
        };
        let label = manifest
            .label
            .unwrap_or_else(|| crate::node::service::label("hub"));
        let app = manifest.app.or_else(|| {
            let name = platform?.app;
            let mut places = vec![dir.join(name)];
            if cfg!(target_os = "macos") {
                places.push(PathBuf::from("/Applications").join(name));
                places.push(user_home.join("Applications").join(name));
            }
            places.into_iter().find(|place| place.exists())
        });
        let service = manifest.service.unwrap_or_else(|| {
            if cfg!(target_os = "macos")
                && user_home
                    .join(format!("Library/LaunchAgents/{label}.plist"))
                    .exists()
            {
                Service::Launchd
            } else if cfg!(target_os = "linux")
                && user_home
                    .join(format!(".config/systemd/user/{label}.service"))
                    .exists()
            {
                Service::Systemd
            } else {
                Service::None
            }
        });
        Ok(Self {
            daemon,
            app,
            service,
            label,
        })
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn updates_dir(home: &Path) -> PathBuf {
    home.join("updates")
}

pub fn read_progress(home: &Path) -> Option<UpdateProgress> {
    let text = fs::read_to_string(updates_dir(home).join("status.json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_progress(home: &Path, version: &str, step: UpdateStep, message: &str) {
    let progress = UpdateProgress {
        version: version.into(),
        step,
        message: message.into(),
        at: now(),
    };
    let dir = updates_dir(home);
    let _ = fs::create_dir_all(&dir);
    let text = serde_json::to_string_pretty(&progress).expect("avancement sérialisable");
    // Écrit à côté puis renommé : le daemon ne lit jamais un fichier à moitié écrit.
    let temporary = dir.join("status.json.tmp");
    if fs::write(&temporary, text).is_ok() {
        let _ = fs::rename(temporary, dir.join("status.json"));
    }
    println!("[{step:?}] {message}");
}

/// Une installation est-elle en cours (et vivante) ?
fn busy(home: &Path) -> bool {
    read_progress(home).is_some_and(|progress| {
        !progress.step.finished() && now().saturating_sub(progress.at) < STALE.as_secs()
    })
}

/// Dernière recherche : quand, et ce qu'elle a trouvé.
type Found = (Instant, Result<Option<Candidate>, String>);

/// Côté daemon : recherche mise en cache, et lancement de l'installation.
pub struct Updates {
    home: PathBuf,
    cache: Mutex<Option<Found>>,
}

impl Updates {
    pub fn new(home: PathBuf) -> Self {
        Self {
            home,
            cache: Mutex::new(None),
        }
    }

    pub async fn status(&self, refresh: bool) -> UpdateStatus {
        let cached = self
            .cache
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(at, found)| !refresh && found.is_ok() && at.elapsed() < CHECK_TTL)
            .map(|(_, found)| found.clone());
        let found = match cached {
            Some(found) => found,
            None => {
                let found = find().await;
                *self.cache.lock().unwrap() = Some((Instant::now(), found.clone()));
                found
            }
        };
        let (available, error) = match found {
            Ok(candidate) => (candidate.map(|candidate| candidate.info()), None),
            Err(error) => (None, Some(error)),
        };
        UpdateStatus {
            current: env!("CARGO_PKG_VERSION").into(),
            available,
            error,
            progress: read_progress(&self.home),
        }
    }

    /// Lance `argos-daemon update apply` dans un process détaché : il survit à l'arrêt de
    /// ce daemon, qu'il relance une fois les fichiers remplacés.
    pub fn apply(&self) -> Result<(), String> {
        if busy(&self.home) {
            return Err("une mise à jour est déjà en cours".into());
        }
        let dir = updates_dir(&self.home);
        fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("update.log"))
            .map_err(|error| error.to_string())?;
        let program = std::env::current_exe().map_err(|error| error.to_string())?;
        let mut command = Command::new(program);
        command
            .args(["update", "apply", "--restart-pid"])
            .arg(std::process::id().to_string())
            .env("ARGOS_HOME", &self.home)
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|error| error.to_string())?)
            .stderr(log);
        detach(&mut command);
        write_progress(
            &self.home,
            "",
            UpdateStep::Downloading,
            "recherche de la dernière version",
        );
        command
            .spawn()
            .map(drop)
            .map_err(|error| format!("lancement de la mise à jour : {error}"))
    }
}

/// Hors du groupe de process du daemon : launchd et systemd tuent le groupe entier quand
/// ils relancent le service.
fn detach(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
}

/// Options de `argos-daemon update apply`.
#[derive(Debug, Default)]
pub struct ApplyOptions {
    /// Daemon à arrêter puis relancer quand aucun service ne le gère (lancé par la console).
    pub restart_pid: Option<u32>,
}

/// Installe la dernière version publiée. Rend ce qui a été fait, pour l'utilisateur.
pub async fn apply(home: &Path, options: ApplyOptions) -> Result<String, String> {
    let platform = platform().ok_or("pas d'archive publiée pour cette plateforme")?;
    let layout = Layout::detect().map_err(|error| error.to_string())?;
    let Some(candidate) = find().await.inspect_err(|error| {
        write_progress(home, "", UpdateStep::Failed, error);
    })?
    else {
        let message = format!("Argos {} est à jour", env!("CARGO_PKG_VERSION"));
        write_progress(home, env!("CARGO_PKG_VERSION"), UpdateStep::Done, &message);
        return Ok(message);
    };
    let version = candidate.version.to_string();
    let result = install_candidate(home, &layout, platform, &candidate, &options).await;
    // Archive et fichiers décompressés ne servent plus, réussite ou échec.
    let _ = fs::remove_dir_all(updates_dir(home).join(&version));
    match &result {
        Ok(message) => write_progress(home, &version, UpdateStep::Done, message),
        Err(error) => write_progress(home, &version, UpdateStep::Failed, error),
    }
    result
}

async fn install_candidate(
    home: &Path,
    layout: &Layout,
    platform: Platform,
    candidate: &Candidate,
    options: &ApplyOptions,
) -> Result<String, String> {
    let version = candidate.version.to_string();
    let work = updates_dir(home).join(&version);
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|error| error.to_string())?;

    write_progress(
        home,
        &version,
        UpdateStep::Downloading,
        &format!("téléchargement d'Argos {version}"),
    );
    let sums_path = work.join(platform.sums);
    download(&candidate.sums_url, &sums_path).await?;
    let archive = work.join(platform.archive);
    download(&candidate.archive_url, &archive).await?;
    let sums = fs::read_to_string(&sums_path).map_err(|error| error.to_string())?;
    let expected = expected_sum(&sums, platform.archive)
        .ok_or_else(|| format!("{} absent de {}", platform.archive, platform.sums))?;
    let actual = sha256_file(&archive).map_err(|error| error.to_string())?;
    if actual != expected {
        return Err(format!(
            "somme SHA-256 incorrecte pour {} : archive corrompue ou modifiée",
            platform.archive
        ));
    }

    write_progress(
        home,
        &version,
        UpdateStep::Installing,
        &format!("installation d'Argos {version}"),
    );
    let extracted = work.join("extrait");
    extract(&archive, &extracted)?;
    let root = archive_root(&extracted, platform)?;
    // Le nouveau daemon doit démarrer et annoncer la version attendue avant qu'on touche
    // à quoi que ce soit : une archive d'une autre architecture s'arrête ici.
    let announced = run_version(&root.join(platform.daemon))?;
    if announced != version {
        return Err(format!(
            "l'archive annonce Argos {version} mais contient la version {announced}"
        ));
    }
    let replaced = install(&root, layout, platform).map_err(|error| error.to_string())?;
    migrate_config(&layout.daemon, home)?;

    write_progress(home, &version, UpdateStep::Restarting, "relance du daemon");
    let restarted = restart(layout, home, options.restart_pid)?;
    Ok(format!(
        "Argos {version} installé ({}) ; {restarted}",
        replaced.join(", ")
    ))
}

/// Décompresse avec les outils du système : `ditto` garde les attributs d'un bundle macOS,
/// `tar` (bsdtar sous Windows 10 et plus) lit aussi les ZIP.
pub fn extract(archive: &Path, into: &Path) -> Result<(), String> {
    fs::create_dir_all(into).map_err(|error| error.to_string())?;
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("ditto");
        command.args(["-x", "-k"]).arg(archive).arg(into);
        command
    } else {
        let mut command = Command::new("tar");
        command.arg("-xf").arg(archive).arg("-C").arg(into);
        command
    };
    let status = command
        .status()
        .map_err(|error| format!("décompression : {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("décompression de {} échouée", archive.display()))
}

/// Le dossier de l'archive qui contient le daemon : la racine, ou `Argos/` sous Linux.
pub fn archive_root(extracted: &Path, platform: Platform) -> Result<PathBuf, String> {
    [extracted.to_path_buf(), extracted.join("Argos")]
        .into_iter()
        .find(|dir| dir.join(platform.daemon).is_file())
        .ok_or_else(|| format!("{} absent de l'archive", platform.daemon))
}

/// `argos-daemon version` d'un binaire.
fn run_version(daemon: &Path) -> Result<String, String> {
    let output = Command::new(daemon)
        .arg("version")
        .output()
        .map_err(|error| format!("le nouveau daemon ne démarre pas : {error}"))?;
    if !output.status.success() {
        return Err("le nouveau daemon ne démarre pas".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Remplace l'app, les fichiers déjà présents à côté du daemon (lanceurs, voix
/// embarquée sous Windows), puis le daemon en dernier. Rend ce qui a été remplacé.
pub fn install(root: &Path, layout: &Layout, platform: Platform) -> io::Result<Vec<String>> {
    let mut replaced = Vec::new();
    if let Some(app) = &layout.app {
        let source = root.join(platform.app);
        if source.exists() {
            replace(&source, app, false)?;
            replaced.push(format!("app {}", app.display()));
        }
    }
    let dir = layout.daemon.parent().unwrap_or(Path::new("."));
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let target = dir.join(&name);
        if name == platform.daemon || name == platform.app || !target.exists() {
            continue;
        }
        if Some(&target) == layout.app.as_ref() {
            continue;
        }
        replace(&entry.path(), &target, false)?;
        replaced.push(name.to_string_lossy().into_owned());
    }
    // L'ancien daemon reste à côté (`.previous`) pour revenir en arrière à la main.
    replace(&root.join(platform.daemon), &layout.daemon, true)?;
    replaced.push(format!("daemon {}", layout.daemon.display()));
    Ok(replaced)
}

/// Remplace `target` par `source`. Un bundle macOS est copié à côté puis échangé d'un
/// renommage ; un dossier ordinaire l'est fichier par fichier.
fn replace(source: &Path, target: &Path, keep_previous: bool) -> io::Result<()> {
    if source.is_dir() {
        if cfg!(target_os = "macos") && target.extension().is_some_and(|ext| ext == "app") {
            return replace_bundle(source, target);
        }
        fs::create_dir_all(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            replace(&entry.path(), &target.join(entry.file_name()), false)?;
        }
        return Ok(());
    }
    replace_file(source, target, keep_previous)
}

fn sibling(target: &Path, suffix: &str) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    target.with_file_name(name)
}

/// Copie à côté puis renomme : jamais de fichier à moitié écrit à la place de l'ancien.
/// L'ancien est d'abord écarté (`.previous`), ce que Windows permet même pour un
/// exécutable en cours d'exécution, contrairement à l'écrasement.
fn replace_file(source: &Path, target: &Path, keep_previous: bool) -> io::Result<()> {
    let fresh = sibling(target, ".new");
    let previous = sibling(target, ".previous");
    fs::copy(source, &fresh)?;
    if target.exists() {
        let _ = fs::remove_file(&previous);
        fs::rename(target, &previous)?;
    }
    if let Err(error) = fs::rename(&fresh, target) {
        let _ = fs::rename(&previous, target);
        return Err(error);
    }
    if !keep_previous {
        // Un exécutable encore en marche sous Windows ne s'efface pas : il partira la
        // prochaine fois.
        let _ = fs::remove_file(&previous);
    }
    Ok(())
}

fn replace_bundle(source: &Path, target: &Path) -> io::Result<()> {
    let dir = target.parent().unwrap_or(Path::new("."));
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let fresh = dir.join(format!(".{name}.new"));
    let previous = dir.join(format!(".{name}.previous"));
    let _ = fs::remove_dir_all(&fresh);
    let status = Command::new("ditto").arg(source).arg(&fresh).status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "copie de {} échouée",
            source.display()
        )));
    }
    // Une app copiée par un process sans quarantaine n'en a pas ; au cas où l'archive
    // en portait une.
    let _ = Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&fresh)
        .status();
    let _ = fs::remove_dir_all(&previous);
    if target.exists() {
        fs::rename(target, &previous)?;
    }
    if let Err(error) = fs::rename(&fresh, target) {
        let _ = fs::rename(&previous, target);
        return Err(error);
    }
    let _ = fs::remove_dir_all(&previous);
    Ok(())
}

/// Le nouveau daemon migre la configuration : lui seul connaît les nouveaux formats.
fn migrate_config(daemon: &Path, home: &Path) -> Result<(), String> {
    let output = Command::new(daemon)
        .args(["config", "init"])
        .env("ARGOS_HOME", home)
        .output()
        .map_err(|error| format!("migration de la configuration : {error}"))?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "migration de la configuration : {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Relance le daemon par son service ; sans service, arrête celui qui a demandé la mise à
/// jour et en démarre un nouveau, détaché.
fn restart(layout: &Layout, home: &Path, pid: Option<u32>) -> Result<String, String> {
    let run = |program: &str, args: &[&str]| -> Result<(), String> {
        let status = Command::new(program)
            .args(args)
            .status()
            .map_err(|error| format!("{program} : {error}"))?;
        status
            .success()
            .then_some(())
            .ok_or_else(|| format!("{program} {} a échoué", args.join(" ")))
    };
    match layout.service {
        Service::Launchd => {
            let target = format!("gui/{}/{}", user_id(), layout.label);
            run("launchctl", &["kickstart", "-k", &target])?;
            Ok(format!("service launchd {} relancé", layout.label))
        }
        Service::Systemd => {
            run("systemctl", &["--user", "restart", &layout.label])?;
            Ok(format!("service systemd {} relancé", layout.label))
        }
        Service::None => {
            let Some(pid) = pid else {
                return Ok("relancez le daemon pour passer à la nouvelle version".into());
            };
            stop(pid);
            let log = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(home.join("daemon.log"))
                .map_err(|error| error.to_string())?;
            let mut command = Command::new(&layout.daemon);
            command
                .arg("hub")
                .env("ARGOS_HOME", home)
                .stdin(Stdio::null())
                .stdout(log.try_clone().map_err(|error| error.to_string())?)
                .stderr(log);
            detach(&mut command);
            command
                .spawn()
                .map_err(|error| format!("relance du daemon : {error}"))?;
            Ok("daemon relancé".into())
        }
    }
}

#[cfg(unix)]
fn user_id() -> u32 {
    // SAFETY : getuid ne peut pas échouer.
    unsafe { libc::getuid() }
}

#[cfg(not(unix))]
fn user_id() -> u32 {
    0
}

/// Arrête le daemon comme un Ctrl+C (il parke ses sessions), puis attend sa fin.
#[cfg(unix)]
fn stop(pid: u32) {
    let pid = pid as libc::pid_t;
    // SAFETY : simples appels système sur un pid.
    unsafe {
        libc::kill(pid, libc::SIGINT);
        for _ in 0..120 {
            if libc::kill(pid, 0) != 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        libc::kill(pid, libc::SIGKILL);
    }
    std::thread::sleep(Duration::from_millis(500));
}

#[cfg(windows)]
fn stop(pid: u32) {
    let _ = Command::new("taskkill")
        // Sans /T : ce process est lui-même un enfant du daemon.
        .args(["/PID", &pid.to_string(), "/F"])
        .status();
    std::thread::sleep(Duration::from_secs(2));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, assets: &[&str]) -> GithubRelease {
        GithubRelease {
            tag_name: tag.into(),
            draft: false,
            body: Some(format!("notes {tag}")),
            html_url: format!("https://example.test/{tag}"),
            assets: assets
                .iter()
                .map(|name| GithubAsset {
                    name: (*name).into(),
                    browser_download_url: format!("https://example.test/{tag}/{name}"),
                })
                .collect(),
        }
    }

    #[test]
    fn the_newest_release_with_this_platform_wins() {
        let mac = platform_for("macos", "aarch64").unwrap();
        let both = [mac.archive, mac.sums];
        let mut draft = release("v0.9.0", &both);
        draft.draft = true;
        let releases = [
            draft,
            release(
                "v0.5.0",
                &["Argos-Windows-x64.zip", "SHA256SUMS-windows.txt"],
            ),
            release("v0.4.1", &both),
            release("v0.4.0-rc.1", &both),
            release("pas-une-version", &both),
            release("v0.3.0", &both),
        ];
        let current = Version::parse("0.3.0").unwrap();
        let found = newest(&releases, &current, mac).unwrap();
        assert_eq!(found.version, Version::parse("0.4.1").unwrap());
        assert_eq!(
            found.archive_url,
            "https://example.test/v0.4.1/Argos-macOS-arm64.zip"
        );
        assert_eq!(found.info().notes, "notes v0.4.1");

        let latest = Version::parse("0.4.1").unwrap();
        assert_eq!(newest(&releases, &latest, mac), None);
        // Une archive sans son fichier de sommes n'est pas installable.
        assert_eq!(
            newest(&[release("v1.0.0", &[mac.archive])], &current, mac),
            None
        );
    }

    #[test]
    fn every_published_archive_has_a_platform() {
        let names: Vec<_> = [
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("linux", "x86_64"),
            ("windows", "x86_64"),
        ]
        .iter()
        .map(|(os, arch)| platform_for(os, arch).unwrap().archive)
        .collect();
        // Les noms de `.github/workflows/release.yml`.
        let workflow = include_str!("../../../.github/workflows/release.yml");
        for name in names {
            assert!(workflow.contains(name), "{name} absent du workflow");
        }
        assert_eq!(platform_for("linux", "aarch64"), None);
    }

    #[test]
    fn sums_are_read_by_file_name() {
        let hash = "a".repeat(64);
        let sums = format!(
            "{hash}  Argos-macOS-arm64.zip\n{}  autre.zip\n",
            "b".repeat(64)
        );
        assert_eq!(expected_sum(&sums, "Argos-macOS-arm64.zip"), Some(hash));
        assert_eq!(expected_sum(&sums, "absent.zip"), None);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, "abc").unwrap();
        assert_eq!(
            sha256_file(&file).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn the_layout_comes_from_the_manifest_or_the_usual_places() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let daemon = bin.join("argos-daemon");
        let linux = platform_for("linux", "x86_64");

        let bare = Layout::for_daemon(daemon.clone(), dir.path(), linux).unwrap();
        assert_eq!(bare.app, None);
        assert_eq!(bare.service, Service::None);
        assert_eq!(bare.label, "argos-hub");

        // Archive décompressée : l'app à côté du daemon.
        fs::write(bin.join("Argos.AppImage"), "").unwrap();
        let unpacked = Layout::for_daemon(daemon.clone(), dir.path(), linux).unwrap();
        assert_eq!(unpacked.app, Some(bin.join("Argos.AppImage")));

        fs::write(
            bin.join(MANIFEST),
            "app = \"/opt/Argos.AppImage\"\nservice = \"systemd\"\nlabel = \"argos-essai\"\n",
        )
        .unwrap();
        let declared = Layout::for_daemon(daemon, dir.path(), linux).unwrap();
        assert_eq!(declared.app, Some(PathBuf::from("/opt/Argos.AppImage")));
        assert_eq!(declared.service, Service::Systemd);
        assert_eq!(declared.label, "argos-essai");
    }

    #[test]
    fn installing_replaces_the_app_the_neighbours_and_the_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let platform = platform_for("linux", "x86_64").unwrap();
        let staged = dir.path().join("extrait/Argos");
        fs::create_dir_all(&staged).unwrap();
        for (name, text) in [
            ("argos-daemon", "nouveau daemon"),
            ("Argos.AppImage", "nouvelle app"),
            ("lancer-argos.sh", "nouveau lanceur"),
            ("LISEZ-MOI.txt", "absent de l'installation"),
        ] {
            fs::write(staged.join(name), text).unwrap();
        }
        let installed = dir.path().join("installé");
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("argos-daemon"), "ancien daemon").unwrap();
        fs::write(installed.join("lancer-argos.sh"), "ancien lanceur").unwrap();
        let app = dir.path().join("ailleurs/Argos.AppImage");
        fs::create_dir_all(app.parent().unwrap()).unwrap();
        fs::write(&app, "ancienne app").unwrap();
        let layout = Layout {
            daemon: installed.join("argos-daemon"),
            app: Some(app.clone()),
            service: Service::None,
            label: "argos-hub".into(),
        };

        let root = archive_root(&dir.path().join("extrait"), platform).unwrap();
        assert_eq!(root, staged);
        let replaced = install(&root, &layout, platform).unwrap();
        assert_eq!(replaced.len(), 3, "{replaced:?}");
        let read = |path: PathBuf| fs::read_to_string(path).unwrap();
        assert_eq!(read(app), "nouvelle app");
        assert_eq!(read(installed.join("lancer-argos.sh")), "nouveau lanceur");
        assert_eq!(read(installed.join("argos-daemon")), "nouveau daemon");
        assert_eq!(
            read(installed.join("argos-daemon.previous")),
            "ancien daemon"
        );
        assert!(!installed.join("LISEZ-MOI.txt").exists());
        assert!(!installed.join("lancer-argos.sh.previous").exists());
    }

    #[test]
    fn progress_survives_on_disk_and_marks_a_live_install_busy() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_progress(dir.path()), None);
        assert!(!busy(dir.path()));
        write_progress(dir.path(), "0.4.0", UpdateStep::Installing, "installation");
        assert!(busy(dir.path()));
        write_progress(dir.path(), "0.4.0", UpdateStep::Done, "fini");
        let progress = read_progress(dir.path()).unwrap();
        assert_eq!(progress.step, UpdateStep::Done);
        assert_eq!(progress.version, "0.4.0");
        assert!(!busy(dir.path()));
    }
}
