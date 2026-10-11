#requires -Version 5.1
<#
.SYNOPSIS
    Build, install, and run Windows EndlessVibe without locking Cargo's EXE.
.DESCRIPTION
    The running binary is always an immutable copy under the user's
    LocalAppData\Programs\EndlessVibe\releases directory, never under target.
    Update builds into target\windows-stage, safely stops the previous instance,
    and starts the new copy. It never force-kills or modifies configuration.
.EXAMPLE
    .\deploy\windows\EndlessVibe.ps1 -Action Update
.EXAMPLE
    .\deploy\windows\EndlessVibe.ps1 -Action Start
.EXAMPLE
    .\deploy\windows\EndlessVibe.ps1 -Action Update -ConfigPath C:\path\config.toml
#>
[CmdletBinding()]
param(
    [ValidateSet("Update", "Start", "Stop", "Status")]
    [string]$Action = "Update",
    [switch]$Release,
    [string]$ConfigPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not $env:LOCALAPPDATA) {
    throw "LOCALAPPDATA is not set; this script requires a Windows user session."
}

$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..\..")).Path
$installRoot = Join-Path $env:LOCALAPPDATA "Programs\EndlessVibe"
$releasesRoot = Join-Path $installRoot "releases"
$currentFile = Join-Path $installRoot "current.json"
$stageRoot = Join-Path $repo "target\windows-stage"
$profileName = if ($Release) { "release" } else { "debug" }

function Read-Installation {
    if (-not (Test-Path -LiteralPath $currentFile -PathType Leaf)) {
        return $null
    }
    $manifest = Get-Content -LiteralPath $currentFile -Raw | ConvertFrom-Json
    $digest = [string]$manifest.sha256
    if ($digest -notmatch '^[a-fA-F0-9]{64}$') {
        throw "Invalid installed EndlessVibe manifest checksum."
    }
    $exe = Join-Path (Join-Path $releasesRoot $digest.ToLowerInvariant()) "endlessvibe.exe"
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
        throw "The installed EndlessVibe executable is missing: $exe"
    }
    $actual = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash
    if ($actual -ine $digest) {
        throw "Installed EndlessVibe executable checksum mismatch; refusing to run it."
    }
    return [pscustomobject]@{
        Exe = $exe
        Config = [string]$manifest.config_path
    }
}

function Invoke-ServiceControl {
    param(
        [Parameter(Mandatory=$true)][string]$Exe,
        [Parameter(Mandatory=$true)][ValidateSet("status", "stop")][string]$Verb,
        [string]$Configuration
    )
    $arguments = @()
    if ($Configuration) {
        $arguments += "--config"
        $arguments += $Configuration
    }
    $arguments += "--$Verb"
    $output = & $Exe @arguments 2>&1
    $exitCode = $LASTEXITCODE
    $message = ($output | Out-String).Trim()
    if ($exitCode -ne 0) {
        throw "EndlessVibe --$Verb failed (exit $exitCode): $message. Stop the existing service manually if it predates process control or runs under a different account/session; no force-kill was attempted."
    }
    return $message
}

function Start-Installed {
    param([string]$Exe, [string]$Configuration, [switch]$RequireStopped)
    $state = Invoke-ServiceControl -Exe $Exe -Verb "status" -Configuration $Configuration
    if ($state -match 'running pid=') {
        if ($RequireStopped) {
            throw "EndlessVibe was restarted before deployment could launch the new copy. Update the auto-start service/task to use this script with -Action Start."
        }
        Write-Host "EndlessVibe is already running: $state"
        return
    }
    if ($state -match 'legacy_instance') {
        throw "An older instance still holds the service lock. Stop it with its service manager first."
    }

    $options = @{
        FilePath = $Exe
        WorkingDirectory = $repo
        PassThru = $true
    }
    if ($Configuration) {
        # Start-Process joins argument arrays: quote a path containing spaces.
        $options.ArgumentList = '--config "' + $Configuration + '"'
    }
    $child = Start-Process @options
    for ($attempt = 0; $attempt -lt 30; $attempt++) {
        Start-Sleep -Milliseconds 250
        $state = Invoke-ServiceControl -Exe $Exe -Verb "status" -Configuration $Configuration
        if ($state -match 'running pid=([0-9]+)') {
            if ([int]$Matches[1] -ne $child.Id) {
                throw "A different EndlessVibe process is running (PID $($Matches[1])). Check the existing service supervisor; do not start the Cargo target EXE."
            }
            Write-Host "EndlessVibe started: $state"
            Write-Host "Running executable: $Exe"
            return
        }
        if ($child.HasExited) {
            throw "The installed EndlessVibe process exited immediately (exit code $($child.ExitCode)). Review its console output or service configuration."
        }
    }
    throw "EndlessVibe did not report a running instance. Check its service log and configuration."
}

$previous = Read-Installation
$previousConfiguration = if ($null -ne $previous) { $previous.Config } else { $ConfigPath }
if (-not $ConfigPath -and $null -ne $previous) {
    $ConfigPath = $previous.Config
}
if ($ConfigPath) {
    if (-not [System.IO.Path]::IsPathRooted($ConfigPath)) {
        throw "ConfigPath must be an absolute Windows path."
    }
    $ConfigPath = [System.IO.Path]::GetFullPath($ConfigPath)
    if (-not (Test-Path -LiteralPath $ConfigPath -PathType Leaf)) {
        throw "ConfigPath does not exist: $ConfigPath"
    }
}

switch ($Action) {
    "Start" {
        if ($null -eq $previous) {
            throw "No installed EndlessVibe binary. Run -Action Update first."
        }
        Start-Installed -Exe $previous.Exe -Configuration $ConfigPath
        break
    }
    "Stop" {
        if ($null -eq $previous) {
            throw "No installed EndlessVibe binary. Run -Action Update first."
        }
        Write-Host (Invoke-ServiceControl -Exe $previous.Exe -Verb "stop" -Configuration $ConfigPath)
        break
    }
    "Status" {
        if ($null -eq $previous) {
            throw "No installed EndlessVibe binary. Run -Action Update first."
        }
        Write-Host (Invoke-ServiceControl -Exe $previous.Exe -Verb "status" -Configuration $ConfigPath)
        Write-Host "Installed executable: $($previous.Exe)"
        break
    }
    "Update" {
        Push-Location $repo
        try {
            Write-Host "Building in the isolated Cargo output directory..."
            $cargoArguments = @("build", "--locked", "--target-dir", $stageRoot)
            if ($Release) { $cargoArguments += "--release" }
            & cargo @cargoArguments
            if ($LASTEXITCODE -ne 0) {
                throw "Cargo build failed. The existing EndlessVibe service was left running."
            }
        }
        finally {
            Pop-Location
        }

        $source = Join-Path (Join-Path $stageRoot $profileName) "endlessvibe.exe"
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
            throw "Cargo reported success but the executable is missing: $source"
        }
        $digest = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
        $releaseDirectory = Join-Path $releasesRoot $digest
        $installed = Join-Path $releaseDirectory "endlessvibe.exe"
        New-Item -ItemType Directory -Path $releaseDirectory -Force | Out-Null

        if (-not (Test-Path -LiteralPath $installed -PathType Leaf)) {
            $temporary = Join-Path $releaseDirectory ("endlessvibe-" + [guid]::NewGuid().ToString("N") + ".tmp")
            Copy-Item -LiteralPath $source -Destination $temporary
            try {
                Move-Item -LiteralPath $temporary -Destination $installed
            }
            finally {
                if (Test-Path -LiteralPath $temporary) {
                    Remove-Item -LiteralPath $temporary -Force
                }
            }
        }
        if ((Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash -ine $digest) {
            throw "Installed executable checksum mismatch. The running service was not stopped."
        }

        # Stop through EndlessVibe's verified PID + creation-time Windows event.
        # A legacy process, protected session, or supervisor must be stopped manually.
        $controller = if ($null -ne $previous) { $previous.Exe } else { $installed }
        Write-Host (Invoke-ServiceControl -Exe $controller -Verb "stop" -Configuration $previousConfiguration)

        $manifest = @{ sha256 = $digest; config_path = $ConfigPath } | ConvertTo-Json -Compress
        New-Item -ItemType Directory -Path $installRoot -Force | Out-Null
        $temporaryManifest = Join-Path $installRoot ("current-" + [guid]::NewGuid().ToString("N") + ".tmp")
        [System.IO.File]::WriteAllText($temporaryManifest, $manifest,
            ([System.Text.UTF8Encoding]::new($false)))
        try {
            if (Test-Path -LiteralPath $currentFile) {
                [System.IO.File]::Replace($temporaryManifest, $currentFile, $null)
            }
            else {
                Move-Item -LiteralPath $temporaryManifest -Destination $currentFile
            }
        }
        finally {
            if (Test-Path -LiteralPath $temporaryManifest) {
                Remove-Item -LiteralPath $temporaryManifest -Force
            }
        }
        Start-Installed -Exe $installed -Configuration $ConfigPath -RequireStopped
        Write-Host "Done. Regular 'cargo build' can now overwrite target\debug\endlessvibe.exe because the service runs outside target."
        break
    }
}
