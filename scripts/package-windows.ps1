$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
function Download-Checked([string]$url, [string]$path, [string]$sha256) {
    Invoke-WebRequest -Uri $url -OutFile $path
    $actual = (Get-FileHash -Algorithm SHA256 $path).Hash.ToLowerInvariant()
    if ($actual -ne $sha256) { Remove-Item $path -Force; throw "Somme SHA-256 incorrecte : $url" }
}

cargo build --locked --release -p argos-daemon
if ($LASTEXITCODE -ne 0) { throw 'La compilation du daemon a échoué.' }
Push-Location (Join-Path $root 'apps/gui')
try {
    npm ci
    if ($LASTEXITCODE -ne 0) { throw 'npm ci a échoué.' }
    npm run tauri build -- --bundles nsis
    if ($LASTEXITCODE -ne 0) { throw 'La compilation de l''application a échoué.' }
} finally { Pop-Location }

$out = Join-Path $root 'target/distribution/windows-x64'
$stage = Join-Path $root 'target/distribution/windows-stage'
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage, $out, "$stage/python", "$stage/models", "$stage/whisper" | Out-Null
Copy-Item (Join-Path $root 'target/release/argos-daemon.exe') $stage
Copy-Item (Join-Path $root 'target/release/argos-gui.exe') (Join-Path $stage 'Argos.exe')
Copy-Item (Join-Path $root 'scripts/windows/*') $stage

$work = Join-Path $root 'target/distribution/downloads'
New-Item -ItemType Directory -Force $work | Out-Null
Download-Checked 'https://www.python.org/ftp/python/3.12.10/python-3.12.10-embed-amd64.zip' "$work/python.zip" '4acbed6dd1c744b0376e3b1cf57ce906f9dc9e95e68824584c8099a63025a3c3'
Expand-Archive "$work/python.zip" "$stage/python" -Force
Add-Content -Encoding ascii "$stage/python/python312._pth" "`nLib/site-packages`nimport site"
python -m pip install --disable-pip-version-check --target "$stage/python/Lib/site-packages" 'kokoro-onnx==0.6.1' 'sounddevice==0.5.6'
if ($LASTEXITCODE -ne 0) { throw 'Installation des dépendances Kokoro échouée.' }
Download-Checked 'https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-x64.zip' "$work/whisper.zip" '6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb'
Expand-Archive "$work/whisper.zip" "$work/whisper" -Force
Copy-Item "$work/whisper/Release/*" "$stage/whisper/" -Recurse
Download-Checked 'https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.1/kokoro-v1.0.onnx' "$stage/models/kokoro-v1.0.onnx" 'beb0d1848dee9a49da392cc3df26958d46cfa35d321edf434f52949153f0df3a'
Download-Checked 'https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.1/voices-v1.0.bin' "$stage/models/voices-v1.0.bin" 'bca610b8308e8d99f32e6fe4197e7ec01679264efed0cac9140fe9c29f1fbf7d'
Download-Checked 'https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin' "$stage/models/ggml-small.bin" '1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b'
& "$stage/python/python.exe" -c 'import kokoro_onnx, sounddevice; print("Kokoro prêt")'
if ($LASTEXITCODE -ne 0) { throw 'Les dépendances vocales embarquées ne démarrent pas.' }
& "$stage/python/python.exe" -c 'import sys; from kokoro_onnx import Kokoro; k=Kokoro(sys.argv[1],sys.argv[2]); a,r=k.create("Bonjour.", voice="ff_siwis", lang="fr-fr"); assert len(a)>0 and r>0' "$stage/models/kokoro-v1.0.onnx" "$stage/models/voices-v1.0.bin"
if ($LASTEXITCODE -ne 0) { throw 'La synthèse française Kokoro embarquée a échoué.' }
if (-not (Test-Path "$stage/whisper/whisper-server.exe")) { throw 'Serveur Whisper absent.' }
$archive = Join-Path $out 'Argos-Windows-x64.zip'
Remove-Item $archive -Force -ErrorAction SilentlyContinue
Compress-Archive -Path "$stage/*" -DestinationPath $archive -CompressionLevel Fastest
$hash = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLowerInvariant()
"$hash  Argos-Windows-x64.zip" | Set-Content -Encoding utf8 (Join-Path $out 'SHA256SUMS-windows.txt')
Write-Host "Artefact : $archive"
