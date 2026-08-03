[CmdletBinding()]
param(
    [Parameter()]
    [string]$OutputRoot = (Join-Path (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path 'dist'),

    [Parameter()]
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

function Invoke-Native {
    param(
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $true)][string[]]$Arguments
    )

    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed with exit code ${LASTEXITCODE}: $Command $($Arguments -join ' ')"
    }
}

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$cargoManifest = Join-Path $repoRoot 'Cargo.toml'
$versionLine = Select-String -LiteralPath $cargoManifest -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if ($null -eq $versionLine) {
    throw "Could not determine the package version from $cargoManifest"
}
$version = $versionLine.Matches[0].Groups[1].Value

$releaseDirectory = Join-Path $repoRoot 'target\release'
$executable = Join-Path $releaseDirectory 'lore.exe'
if (-not $SkipBuild) {
    Push-Location $repoRoot
    try {
        $previousIncremental = $env:CARGO_INCREMENTAL
        $env:CARGO_INCREMENTAL = '0'
        Invoke-Native -Command 'cargo' -Arguments @('build', '--release', '--locked')
        if ($null -eq $previousIncremental) {
            Remove-Item Env:CARGO_INCREMENTAL -ErrorAction SilentlyContinue
        }
        else {
            $env:CARGO_INCREMENTAL = $previousIncremental
        }
    }
    finally {
        Pop-Location
    }
}

if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw "Release executable was not found: $executable"
}

$resolvedOutputRoot = [System.IO.Path]::GetFullPath($OutputRoot)
New-Item -ItemType Directory -Path $resolvedOutputRoot -Force | Out-Null
$stagingRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("lore-windows-package-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stagingRoot -Force | Out-Null

try {
    Copy-Item -LiteralPath $executable -Destination (Join-Path $stagingRoot 'lore.exe')
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'install.ps1') -Destination (Join-Path $stagingRoot 'install.ps1')
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'install.cmd') -Destination (Join-Path $stagingRoot 'install.cmd')
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'uninstall.ps1') -Destination (Join-Path $stagingRoot 'uninstall.ps1')
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'uninstall.cmd') -Destination (Join-Path $stagingRoot 'uninstall.cmd')
    @"
Lore $version - Windows package

Install interactively:
  powershell -ExecutionPolicy Bypass -File .\install.ps1
  .\install.cmd

Install with an explicit project after reviewing setup output:
  powershell -ExecutionPolicy Bypass -File .\install.ps1 -ProjectPath C:\path\to\repository -ApplySetup

Uninstall:
  .\uninstall.cmd -ProjectPath C:\path\to\repository

The package never scans drives or discovers repositories recursively.
"@ | Set-Content -LiteralPath (Join-Path $stagingRoot 'PACKAGE.txt') -Encoding utf8

    $archive = Join-Path $resolvedOutputRoot ("lore-$version-windows-x86_64.zip")
    if (Test-Path -LiteralPath $archive) {
        Remove-Item -LiteralPath $archive -Force
    }
    Compress-Archive -Path (Join-Path $stagingRoot '*') -DestinationPath $archive -CompressionLevel Optimal
    $hash = Get-FileHash -LiteralPath $archive -Algorithm SHA256
    [pscustomobject]@{
        version = $version
        archive = $archive
        sha256 = $hash.Hash
    } | ConvertTo-Json -Compress
}
finally {
    if (Test-Path -LiteralPath $stagingRoot) {
        Remove-Item -LiteralPath $stagingRoot -Recurse -Force
    }
}
