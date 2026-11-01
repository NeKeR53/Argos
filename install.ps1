# Installe Argos sur Windows x64 depuis la dernière release GitHub (D46).
#
#   irm https://raw.githubusercontent.com/NeKeR53/Argos/main/install.ps1 | iex
#
# Télécharge l'archive et vérifie sa somme SHA-256, l'installe dans
# %LOCALAPPDATA%\Argos\app (données dans %LOCALAPPDATA%\Argos\data), crée les raccourcis,
# puis lance la configuration : confiée à `claude` ou `codex` s'ils sont là, sinon par
# quelques questions. Relancé sur une installation existante, il la met à jour.
#
# Paramètres (avec `& ([scriptblock]::Create((irm …))) -NoAi` ou en lançant le fichier) :
#   -Version vX.Y.Z  -Archive FICHIER [-Sums FICHIER]  -NoAi  -NoSetup  -NoLaunch  -Yes
# Variables pour les essais : ARGOS_RELEASES_URL, ARGOS_INSTALL_DIR, ARGOS_HOME.
param(
    [string]$Version = '',
    [string]$Archive = '',
    [string]$Sums = '',
    [switch]$NoAi,
    [switch]$NoSetup,
    [switch]$NoLaunch,
    [switch]$Yes
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$asset = 'Argos-Windows-x64.zip'
$sumsName = 'SHA256SUMS-windows.txt'
$releasesUrl = if ($env:ARGOS_RELEASES_URL) { $env:ARGOS_RELEASES_URL } else { 'https://api.github.com/repos/NeKeR53/Argos/releases?per_page=20' }
$installDir = if ($env:ARGOS_INSTALL_DIR) { $env:ARGOS_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Argos\app' }
# Le même dossier de données que Lancer-Argos.ps1.
if (-not $env:ARGOS_HOME) { $env:ARGOS_HOME = Join-Path $env:LOCALAPPDATA 'Argos\data' }

function Say([string]$text) { Write-Host $text -ForegroundColor Cyan }
function Ask([string]$question, [string]$default) {
    if ($Yes -or -not [Environment]::UserInteractive) { return $default }
    $answer = Read-Host "$question [$default]"
    if ([string]::IsNullOrWhiteSpace($answer)) { return $default } else { return $answer }
}

$arch = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture
if ($arch -ne [Runtime.InteropServices.Architecture]::X64) {
    throw "Pas d'archive Argos pour Windows $arch (Windows x64 seulement)."
}

$work = Join-Path ([IO.Path]::GetTempPath()) ("argos-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $work | Out-Null
try {
    if (-not $Archive) {
        Say "Recherche de la release d'Argos ($asset)…"
        $releases = Invoke-RestMethod -Uri $releasesUrl -Headers @{ Accept = 'application/vnd.github+json'; 'User-Agent' = 'argos-install' }
        $release = $releases | Where-Object {
            -not $_.draft -and (-not $Version -or $_.tag_name -eq ('v' + $Version.TrimStart('v')))
        } | Where-Object {
            ($_.assets.name -contains $asset) -and ($_.assets.name -contains $sumsName)
        } | Select-Object -First 1
        if (-not $release) { throw "Aucune release ne contient $asset et $sumsName." }
        $archiveUrl = ($release.assets | Where-Object name -eq $asset).browser_download_url
        $sumsUrl = ($release.assets | Where-Object name -eq $sumsName).browser_download_url
        Say "Téléchargement de $archiveUrl"
        $Archive = Join-Path $work $asset
        $Sums = Join-Path $work $sumsName
        Invoke-WebRequest -Uri $archiveUrl -OutFile $Archive
        Invoke-WebRequest -Uri $sumsUrl -OutFile $Sums
    }
    if (-not (Test-Path $Archive)) { throw "Archive introuvable : $Archive" }
    if ($Sums) {
        $line = Get-Content $Sums | Where-Object { $_ -match "\s\*?$([regex]::Escape($asset))$" } | Select-Object -First 1
        if (-not $line) { throw "$asset absent du fichier de sommes." }
        $expected = ($line -split '\s+')[0].ToLowerInvariant()
        $actual = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLowerInvariant()
        if ($actual -ne $expected) { throw 'Somme SHA-256 incorrecte : archive corrompue ou modifiée.' }
        Say 'Somme SHA-256 vérifiée.'
    } else {
        Say 'Archive locale sans fichier de sommes : non vérifiée.'
    }

    $extract = Join-Path $work 'extrait'
    Expand-Archive -Path $Archive -DestinationPath $extract -Force
    $newDaemon = Join-Path $extract 'argos-daemon.exe'
    if (-not (Test-Path $newDaemon)) { throw 'argos-daemon.exe absent de l''archive.' }
    $newVersion = & $newDaemon version 2>$null
    Say "Installation d'Argos $newVersion…"

    # Un daemon ou une app de ce dossier encore en marche gardent leurs fichiers : on les arrête.
    $daemon = Join-Path $installDir 'argos-daemon.exe'
    $app = Join-Path $installDir 'Argos.exe'
    Get-CimInstance Win32_Process -Filter "Name='argos-daemon.exe' OR Name='Argos.exe'" |
        Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($installDir, [StringComparison]::OrdinalIgnoreCase) } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force; Start-Sleep -Milliseconds 500 }
    New-Item -ItemType Directory -Force $installDir | Out-Null
    Copy-Item -Path (Join-Path $extract '*') -Destination $installDir -Recurse -Force

    # Lu par `argos-daemon update` : rien à relancer, l'app démarre le daemon (Argos.cmd).
    $tomlApp = $app.Replace('\', '\\').Replace('"', '\"')
    $manifest = "# Écrit par install.ps1 : lu par « argos-daemon update ».`napp = `"$tomlApp`"`nservice = `"none`"`n"
    [IO.File]::WriteAllText((Join-Path $installDir 'install.toml'), $manifest, [Text.UTF8Encoding]::new($false))

    $shell = New-Object -ComObject WScript.Shell
    $programs = Join-Path ([Environment]::GetFolderPath('Programs')) 'Argos.lnk'
    foreach ($path in @($programs, (Join-Path ([Environment]::GetFolderPath('Desktop')) 'Argos.lnk'))) {
        $link = $shell.CreateShortcut($path)
        $link.TargetPath = Join-Path $installDir 'Argos.cmd'
        $link.WorkingDirectory = $installDir
        $link.IconLocation = $app
        $link.Save()
    }

    New-Item -ItemType Directory -Force $env:ARGOS_HOME | Out-Null
    & $daemon config init
    if ($LASTEXITCODE -ne 0) { throw "Configuration illisible : $env:ARGOS_HOME\config.toml" }
    Say "Argos est installé dans $installDir (données : $env:ARGOS_HOME)."

    if (-not $NoSetup) {
        $ai = if ($NoAi -or $Yes) { $null } else { @('claude', 'codex') | Where-Object { Get-Command $_ -ErrorAction SilentlyContinue } | Select-Object -First 1 }
        $useAi = $ai -and ((Ask "Confier la configuration à $ai (moteur, mot de déclenchement, Telegram) ? (o/n)" 'o') -match '^[oOyY]')
        if ($useAi) {
            # Un long texte multiligne ne traverse pas les lanceurs .cmd de npm : la consigne
            # passe par un fichier.
            $promptFile = Join-Path $env:ARGOS_HOME 'installation-ia.md'
            [IO.File]::WriteAllText($promptFile, ((& $daemon setup prompt) -join "`n"), [Text.UTF8Encoding]::new($false))
            Say "$ai prend la main ; quittez-le quand la configuration est terminée."
            & $ai "Lis le fichier $promptFile et suis ses instructions pour configurer Argos avec moi."

        } else {
            $engines = @('claude', 'codex') | Where-Object { Get-Command $_ -ErrorAction SilentlyContinue }
            if (-not $engines) {
                Say 'Ni claude ni codex n''est installé. Installez-en un puis authentifiez-le :'
                Write-Host '  npm install -g @anthropic-ai/claude-code ; claude'
                Write-Host '  npm install -g @openai/codex ; codex login'
            }
            $engine = if (@($engines).Count -eq 1) { @($engines)[0] } else { Ask 'Moteur de l''Assistant (claude ou codex)' 'claude' }
            $model = if ($engine -eq 'codex') { 'gpt-6.1-sol' } else { $engine = 'claude'; 'claude-opus-5-5' }
            & $daemon config set assistant.engine $engine | Out-Null
            & $daemon config set assistant.model $model | Out-Null
            Say "Assistant : $engine ($model)."
            if ($engine -eq 'codex') {
                & codex login status *> $null
                if ($LASTEXITCODE -ne 0) { Say 'Codex n''est pas authentifié : lancez « codex login ».' }
            }
            Write-Host 'La voix locale est incluse et se configure au premier lancement d''Argos.cmd ;'
            Write-Host 'le mot de déclenchement se règle ensuite dans [voice] wake_word. Telegram : docs/TELEGRAM.md.'
        }
    }

    Write-Host ''
    Write-Host "Mises à jour : « $daemon update » cherche une nouvelle version, « $daemon update apply »"
    Write-Host 'l''installe ; la console les propose aussi (Réglages → Mises à jour).'
    if (-not $NoLaunch) { Start-Process -FilePath (Join-Path $installDir 'Argos.cmd') -WorkingDirectory $installDir }
} finally {
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
}
