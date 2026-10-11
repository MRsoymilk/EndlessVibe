#requires -Version 5.1
# Minimal transaction helpers for EndlessVibe's installed Windows executable.
# No commands run merely from dot-sourcing this script.
Set-StrictMode -Version Latest

function Publish-EndlessVibeManifest {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory=$true)][string]$ManifestPath,
        [Parameter(Mandatory=$true)][string]$Json
    )
    $root = Split-Path -Parent $ManifestPath
    if (-not $root -or -not [System.IO.Path]::IsPathRooted($root)) {
        throw "Installation manifest must have a fully qualified parent directory."
    }
    [System.IO.Directory]::CreateDirectory($root) | Out-Null
    $parsed = $Json | ConvertFrom-Json
    if ([string]$parsed.sha256 -notmatch '^[a-fA-F0-9]{64}$') {
        throw "Installation manifest has no valid executable hash."
    }

    $nonce = [guid]::NewGuid().ToString("N")
    $staged = Join-Path $root ("current-" + $nonce + ".tmp")
    $backup = Join-Path $root ("previous-" + $nonce + ".json")
    $hadPrevious = [System.IO.File]::Exists($ManifestPath)
    [System.IO.File]::WriteAllText(
        $staged, $Json, ([System.Text.UTF8Encoding]::new($false))
    )
    try {
        if ($hadPrevious) {
            # Windows/.NET can reject a null destinationBackupFileName.
            # The backup path must be fully qualified and non-empty.
            [System.IO.File]::Replace($staged, $ManifestPath, $backup)
        }
        else {
            [System.IO.File]::Move($staged, $ManifestPath)
        }
    }
    finally {
        if ([System.IO.File]::Exists($staged)) {
            [System.IO.File]::Delete($staged)
        }
    }
    return [pscustomobject]@{
        Previous = $hadPrevious
        Backup = if ($hadPrevious) { $backup } else { "" }
        Path = $ManifestPath
    }
}

function Restore-EndlessVibeManifest {
    [CmdletBinding()]
    param([Parameter(Mandatory=$true)]$Transaction)
    if ($Transaction.Previous) {
        if (-not [System.IO.File]::Exists($Transaction.Backup)) {
            throw "Cannot recover prior installation manifest: backup is missing."
        }
        if ([System.IO.File]::Exists($Transaction.Path)) {
            $rejected = $Transaction.Path + ".rejected-" + [guid]::NewGuid().ToString("N")
            [System.IO.File]::Replace($Transaction.Backup, $Transaction.Path, $rejected)
            if ([System.IO.File]::Exists($rejected)) {
                [System.IO.File]::Delete($rejected)
            }
        }
        else {
            [System.IO.File]::Move($Transaction.Backup, $Transaction.Path)
        }
    }
    elseif ([System.IO.File]::Exists($Transaction.Path)) {
        [System.IO.File]::Delete($Transaction.Path)
    }
}

function Complete-EndlessVibeManifest {
    [CmdletBinding()]
    param([Parameter(Mandatory=$true)]$Transaction)
    if ($Transaction.Previous -and [System.IO.File]::Exists($Transaction.Backup)) {
        [System.IO.File]::Delete($Transaction.Backup)
    }
}
