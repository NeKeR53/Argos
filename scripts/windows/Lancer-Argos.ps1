$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$env:ARGOS_HOME = Join-Path $env:LOCALAPPDATA 'Argos/data'
$daemon = Join-Path $root 'argos-daemon.exe'
$app = Join-Path $root 'Argos.exe'
if (-not (Test-Path $daemon) -or -not (Test-Path $app)) { throw 'Extrayez toute l''archive ZIP avant de lancer Argos.' }

# Une instance issue de ce même dossier peut être réutilisée, jamais arrêtée ici.
$running = @(Get-CimInstance Win32_Process -Filter "Name='argos-daemon.exe'" | Where-Object { $_.ExecutablePath -eq $daemon })
if ($running.Count -eq 0) {
    $occupied = @(Get-NetTCPConnection -State Listen -LocalPort 7878 -ErrorAction SilentlyContinue)
    if ($occupied.Count -gt 0) { throw 'Le port 7878 est déjà utilisé. Fermez l''autre daemon Argos avant de continuer.' }
    $process = Start-Process -FilePath $daemon -ArgumentList 'hub' -WorkingDirectory $root -WindowStyle Hidden -PassThru
}
$config = Join-Path $env:ARGOS_HOME 'config.toml'
$token = Join-Path $env:ARGOS_HOME 'token'
for ($i = 0; $i -lt 120 -and (-not (Test-Path $config) -or -not (Test-Path $token)); $i++) {
    if ($process -and $process.HasExited) { throw 'Le daemon Argos ne démarre pas. Lancez argos-daemon.exe hub dans un terminal pour voir l''erreur.' }
    Start-Sleep -Milliseconds 250
}
if (-not (Test-Path $config) -or -not (Test-Path $token)) { throw 'Le daemon Argos ne s''est pas initialisé.' }

# Ne jamais remplacer une configuration vocale existante.
$existing = [IO.File]::ReadAllText($config)
if ($existing -notmatch '(?m)^\s*\[voice\]\s*(?:#.*)?$') {
    $python = (Join-Path $root 'python/python.exe').Replace('\', '/')
    $model = (Join-Path $root 'models/kokoro-v1.0.onnx').Replace('\', '/')
    $voices = (Join-Path $root 'models/voices-v1.0.bin').Replace('\', '/')
    $whisper = (Join-Path $root 'whisper/whisper-server.exe').Replace('\', '/')
    $whisperModel = (Join-Path $root 'models/ggml-small.bin').Replace('\', '/')
    foreach ($path in @($python, $model, $voices, $whisper, $whisperModel)) {
        if (-not (Test-Path $path)) { throw "Fichier vocal manquant : $path" }
    }
    # TOML basic strings : échappement des guillemets dans les chemins Windows.
    $python, $model, $voices, $whisper, $whisperModel = @($python, $model, $voices, $whisper, $whisperModel) | ForEach-Object { $_.Replace('"', '\"') }
    $section = @"

[voice]
whisper_model = "$whisperModel"
whisper_server = "$whisper"
language = "fr"
tts_backend = "kokoro"
kokoro_python = "$python"
kokoro_model = "$model"
kokoro_voices = "$voices"
kokoro_voice = "ff_siwis"
"@
    [IO.File]::AppendAllText($config, $section, [Text.UTF8Encoding]::new($false))
}
Start-Process -FilePath $app -WorkingDirectory $root
