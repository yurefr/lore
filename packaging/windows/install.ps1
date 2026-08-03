[CmdletBinding()]
param(
    [Parameter()]
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA 'Lore\bin'),

    [Parameter()]
    [string]$ProjectPath,

    [Parameter()]
    [switch]$ApplySetup,

    [Parameter()]
    [switch]$SkipSetup,

    [Parameter()]
    [switch]$NonInteractive,

    [Parameter()]
    [switch]$NoPathUpdate
)

$ErrorActionPreference = 'Stop'

function Assert-Windows {
    if ($env:OS -ne 'Windows_NT') {
        throw 'The Windows package can only be installed on Windows.'
    }
}

function Resolve-ExistingDirectory {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        throw "Directory does not exist: $Path"
    }

    return [System.IO.Path]::GetFullPath((Resolve-Path -LiteralPath $Path).Path)
}

function Resolve-InstallRoot {
    param([Parameter(Mandatory = $true)][string]$Path)

    $resolved = [System.IO.Path]::GetFullPath($Path)
    $root = [System.IO.Path]::GetPathRoot($resolved)
    if ([string]::Equals($resolved, $root, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "InstallRoot must be a directory below a filesystem root: $resolved"
    }

    return $resolved.TrimEnd('\')
}

function Add-UserPathEntry {
    param([Parameter(Mandatory = $true)][string]$Entry)

    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @()
    if (-not [string]::IsNullOrWhiteSpace($current)) {
        $entries = $current -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    }

    $alreadyPresent = $entries | Where-Object {
        [string]::Equals($_.TrimEnd('\'), $Entry.TrimEnd('\'), [System.StringComparison]::OrdinalIgnoreCase)
    }
    if ($null -eq $alreadyPresent) {
        $updated = @($entries + $Entry) -join ';'
        [Environment]::SetEnvironmentVariable('Path', $updated, 'User')
        Write-Host "Added $Entry to the user PATH. Open a new terminal to use 'lore'."
    }
}

function Resolve-ProjectSelection {
    param(
        [string]$RequestedPath,
        [bool]$Interactive
    )

    if (-not [string]::IsNullOrWhiteSpace($RequestedPath)) {
        return Resolve-ExistingDirectory -Path $RequestedPath
    }

    $current = (Get-Location).Path
    $hasGitMarker = Test-Path -LiteralPath (Join-Path $current '.git')
    if (-not $Interactive) {
        if ($hasGitMarker) {
            return Resolve-ExistingDirectory -Path $current
        }

        return $null
    }

    if ($hasGitMarker) {
        $answer = Read-Host "Configure Lore for the current Git project '$current'? [Y/n]"
        if ([string]::IsNullOrWhiteSpace($answer) -or $answer -match '^(y|yes)$') {
            return Resolve-ExistingDirectory -Path $current
        }
    }

    $entered = Read-Host 'Optional project path (leave empty to configure MCP only)'
    if ([string]::IsNullOrWhiteSpace($entered)) {
        return $null
    }

    return Resolve-ExistingDirectory -Path $entered
}

function Invoke-Lore {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter()][switch]$FailOnReportErrors
    )

    $capturePath = Join-Path ([System.IO.Path]::GetTempPath()) ("lore-installer-" + [Guid]::NewGuid().ToString('N') + '.json')
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

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$sourceExecutable = Join-Path $scriptRoot 'lore.exe'
if (-not (Test-Path -LiteralPath $sourceExecutable -PathType Leaf)) {
    throw "lore.exe was not found next to the installer: $sourceExecutable"
}

$resolvedInstallRoot = Resolve-InstallRoot -Path $InstallRoot
New-Item -ItemType Directory -Path $resolvedInstallRoot -Force | Out-Null
$targetExecutable = Join-Path $resolvedInstallRoot 'lore.exe'
$temporaryExecutable = "$targetExecutable.$([Guid]::NewGuid().ToString('N')).new"

try {
    Copy-Item -LiteralPath $sourceExecutable -Destination $temporaryExecutable -Force
    Move-Item -LiteralPath $temporaryExecutable -Destination $targetExecutable -Force
}
finally {
    if (Test-Path -LiteralPath $temporaryExecutable) {
        Remove-Item -LiteralPath $temporaryExecutable -Force
    }
}

if (-not $NoPathUpdate) {
    Add-UserPathEntry -Entry $resolvedInstallRoot
}

Write-Host "Installed Lore at $targetExecutable"

$selectedProject = $null
if (-not $SkipSetup) {
    $interactive = -not $NonInteractive
    $selectedProject = Resolve-ProjectSelection -RequestedPath $ProjectPath -Interactive $interactive
    $setupCheckArguments = @('setup', '--check')
    if ($null -ne $selectedProject) {
        $setupCheckArguments += @('--path', $selectedProject)
    }

    Write-Host 'Running a read-only integration check...'
    Invoke-Lore -Executable $targetExecutable -Arguments $setupCheckArguments

    $shouldApply = $ApplySetup
    if (-not $shouldApply -and $interactive) {
        $answer = Read-Host 'Apply Lore MCP configuration (and hooks for the selected project)? [y/N]'
        $shouldApply = $answer -match '^(y|yes)$'
    }

    if ($shouldApply) {
        $setupApplyArguments = @('setup', '--apply', '--yes')
        if ($null -ne $selectedProject) {
            $setupApplyArguments += @('--path', $selectedProject)
        }

        Write-Host 'Applying the confirmed Lore setup...'
        Invoke-Lore -Executable $targetExecutable -Arguments $setupApplyArguments -FailOnReportErrors
    }
    else {
        Write-Host 'Setup changes were not applied. Run lore setup --check and lore setup --apply --yes when ready.'
    }
}

$manifest = [ordered]@{
    install_root = $resolvedInstallRoot
    project_path = $selectedProject
    path_managed = (-not $NoPathUpdate)
    installed_at = [DateTime]::UtcNow.ToString('o')
}
$manifest | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $resolvedInstallRoot 'lore-install.json') -Encoding utf8

Write-Host 'Installation completed. The installer never scans other directories or Git repositories.'
