[CmdletBinding()]
param(
    [Parameter()]
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA 'Lore\bin'),

    [Parameter()]
    [string]$ProjectPath,

    [Parameter()]
    [switch]$SkipSetup,

    [Parameter()]
    [switch]$NoPathUpdate
)

$ErrorActionPreference = 'Stop'

function Assert-Windows {
    if ($env:OS -ne 'Windows_NT') {
        throw 'The Windows package can only be removed on Windows.'
    }
}

function Resolve-InstallRoot {
    param([Parameter(Mandatory = $true)][string]$Path)

    $resolved = [System.IO.Path]::GetFullPath($Path).TrimEnd('\')
    $root = [System.IO.Path]::GetPathRoot($resolved)
    if ([string]::Equals($resolved, $root, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "InstallRoot must be a directory below a filesystem root: $resolved"
    }

    return $resolved
}

function Remove-UserPathEntry {
    param([Parameter(Mandatory = $true)][string]$Entry)

    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ([string]::IsNullOrWhiteSpace($current)) {
        return
    }

    $remaining = $current -split ';' | Where-Object {
        -not [string]::IsNullOrWhiteSpace($_) -and
        -not [string]::Equals($_.TrimEnd('\'), $Entry.TrimEnd('\'), [System.StringComparison]::OrdinalIgnoreCase)
    }
    [Environment]::SetEnvironmentVariable('Path', ($remaining -join ';'), 'User')
}

function Resolve-ExistingDirectory {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        throw "Directory does not exist: $Path"
    }

    return [System.IO.Path]::GetFullPath((Resolve-Path -LiteralPath $Path).Path)
}

function Invoke-Lore {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter()][switch]$FailOnReportErrors
    )

    $capturePath = Join-Path ([System.IO.Path]::GetTempPath()) ("lore-uninstaller-" + [Guid]::NewGuid().ToString('N') + '.json')
    try {
        & $Executable @Arguments > $capturePath
        $exitCode = $LASTEXITCODE
        $output = ''
        if (Test-Path -LiteralPath $capturePath -PathType Leaf) {
            $output = Get-Content -LiteralPath $capturePath -Raw
            if (-not [string]::IsNullOrWhiteSpace($output)) {
                Write-Host $output.TrimEnd()
            }
        }
        if ($exitCode -ne 0) {
            throw "Lore command failed with exit code ${exitCode}: lore $($Arguments -join ' ')"
        }

        if ($FailOnReportErrors -and -not [string]::IsNullOrWhiteSpace($output)) {
            try {
                $report = $output | ConvertFrom-Json
                if (@($report.errors).Count -gt 0) {
                    throw "Lore setup reported errors: $($report.errors -join '; ')"
                }
                return $report
            }
            catch {
                if ($_.Exception.Message.StartsWith('Lore setup reported errors:')) {
                    throw
                }
            }
        }
    }
    finally {
        if (Test-Path -LiteralPath $capturePath) {
            Remove-Item -LiteralPath $capturePath -Force -ErrorAction SilentlyContinue
        }
    }
}

Assert-Windows

$resolvedInstallRoot = Resolve-InstallRoot -Path $InstallRoot
$executable = Join-Path $resolvedInstallRoot 'lore.exe'
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    Write-Host "Lore is not installed at $executable; nothing to remove."
    exit 0
}

$manifestPath = Join-Path $resolvedInstallRoot 'lore-install.json'
$manifest = $null
if (Test-Path -LiteralPath $manifestPath -PathType Leaf) {
    try {
        $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    }
    catch {
        Write-Warning "Install manifest could not be read; continuing without an automatic project target."
    }
}

$project = $null
if (-not [string]::IsNullOrWhiteSpace($ProjectPath)) {
    $project = Resolve-ExistingDirectory -Path $ProjectPath
}
elseif ($null -ne $manifest -and -not [string]::IsNullOrWhiteSpace($manifest.project_path) -and
    (Test-Path -LiteralPath $manifest.project_path -PathType Container)) {
    $project = Resolve-ExistingDirectory -Path $manifest.project_path
}

if (-not $SkipSetup) {
    $setupArguments = @('setup', '--remove', '--yes')
    if ($null -ne $project) {
        $setupArguments += @('--path', $project)
    }

    Write-Host 'Removing only Lore-owned MCP entries and hooks...'
    Invoke-Lore -Executable $executable -Arguments $setupArguments -FailOnReportErrors

    $uninstallArguments = @('uninstall')
    if ($null -ne $project) {
        $uninstallArguments += @('--path', $project)
    }

    Write-Host 'Stopping the local runtime and preserving Lore data by default...'
    Invoke-Lore -Executable $executable -Arguments $uninstallArguments
}

$removeManagedPath = -not $NoPathUpdate
if ($null -ne $manifest -and $manifest.PSObject.Properties.Name -contains 'path_managed' -and
    -not [bool]$manifest.path_managed) {
    $removeManagedPath = $false
}
if ($removeManagedPath) {
    Remove-UserPathEntry -Entry $resolvedInstallRoot
}

Remove-Item -LiteralPath $executable -Force
$manifestPath = Join-Path $resolvedInstallRoot 'lore-install.json'
if (Test-Path -LiteralPath $manifestPath -PathType Leaf) {
    Remove-Item -LiteralPath $manifestPath -Force
}
$remainingFiles = @(Get-ChildItem -LiteralPath $resolvedInstallRoot -Force -ErrorAction SilentlyContinue)
if ($remainingFiles.Count -eq 0 -and (Test-Path -LiteralPath $resolvedInstallRoot -PathType Container)) {
    Remove-Item -LiteralPath $resolvedInstallRoot -Force
}

Write-Host 'Lore binary removed. Project knowledge under LORE_HOME was preserved.'
