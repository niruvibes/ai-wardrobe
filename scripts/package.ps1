$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $projectRoot
try {
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'The release build failed.' }
    $packageFolder = Join-Path $projectRoot 'dist'
    $visionFolder = Join-Path $packageFolder 'vision'
    New-Item -ItemType Directory -Path $visionFolder -Force | Out-Null
    # A versioned executable can be delivered without stopping an older instance
    # that may contain unsaved user edits.
    Copy-Item -LiteralPath (Join-Path $projectRoot 'target\release\wardrobe.exe') -Destination (Join-Path $packageFolder 'Wardrobe-0.3.exe') -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'README.md') -Destination (Join-Path $packageFolder 'README.md') -Force
    foreach ($fileName in @('worker.py', 'gpu_backend.py', 'gpu_setup.py', 'requirements.txt', 'requirements.lock.txt', 'requirements.gpu.txt', 'requirements.gpu.lock.txt')) {
        Copy-Item -LiteralPath (Join-Path $projectRoot "vision\$fileName") -Destination (Join-Path $visionFolder $fileName) -Force
    }
    $docsFolder = Join-Path $packageFolder 'docs'
    New-Item -ItemType Directory -Path $docsFolder -Force | Out-Null
    foreach ($fileName in @('galaxy-s26-ultra-photo-guide.md', 'requirements.md', 'verification.md')) {
        Copy-Item -LiteralPath (Join-Path $projectRoot "docs\$fileName") -Destination (Join-Path $docsFolder $fileName) -Force
    }
    Write-Output (Join-Path $packageFolder 'Wardrobe-0.3.exe')
} finally {
    Pop-Location
}
