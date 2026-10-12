#requires -Version 5.1
# Native PowerShell regression tests for atomic install manifest updates.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot "Manifest.ps1")

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

$root = Join-Path $env:TEMP ("endlessvibe-manifest-" + [guid]::NewGuid().ToString("N"))
[System.IO.Directory]::CreateDirectory($root) | Out-Null
$current = Join-Path $root "current.json"
try {
    $first = @{ sha256 = ("a" * 64); config_path = $null } | ConvertTo-Json -Compress
    $second = @{ sha256 = ("b" * 64); config_path = "C:\config\test.toml" } | ConvertTo-Json -Compress
    $third = @{ sha256 = ("c" * 64); config_path = $null } | ConvertTo-Json -Compress
    $bad = '{"sha256":"invalid"}'

    $initial = Publish-EndlessVibeManifest -ManifestPath $current -Json $first
    Assert-True (-not $initial.Previous) "New installation must not invent a backup"
    Assert-True ([System.IO.File]::ReadAllText($current) -eq $first) "Initial installation corrupted"
    Complete-EndlessVibeManifest -Transaction $initial

    $transaction = Publish-EndlessVibeManifest -ManifestPath $current -Json $second
    Assert-True ($transaction.Previous) "Update must have a prior manifest"
    Assert-True ([System.IO.File]::Exists($transaction.Backup)) "Atomic Replace must save the old manifest"
    Assert-True ([System.IO.File]::ReadAllText($transaction.Backup) -eq $first) "Backup is not the old manifest"
    Assert-True ([System.IO.File]::ReadAllText($current) -eq $second) "Atomic Replace did not publish the new manifest"

    # Simulates failure to launch after graceful stop, requiring rollback.
    Restore-EndlessVibeManifest -Transaction $transaction
    Assert-True ([System.IO.File]::ReadAllText($current) -eq $first) "Rollback failed to restore the old manifest"

    $completed = Publish-EndlessVibeManifest -ManifestPath $current -Json $third
    Complete-EndlessVibeManifest -Transaction $completed
    Assert-True ([System.IO.File]::ReadAllText($current) -eq $third) "Completed update was not retained"
    Assert-True (-not [System.IO.File]::Exists($completed.Backup)) "Backup should be cleared on successful update"

    $invalidRejected = $false
    try { $null = Publish-EndlessVibeManifest -ManifestPath $current -Json $bad }
    catch { $invalidRejected = $true }
    Assert-True $invalidRejected "Invalid manifest digest was not rejected"
    Assert-True ([System.IO.File]::ReadAllText($current) -eq $third) "Invalid manifest modified current installation"

    Assert-True (@([System.IO.Directory]::GetFiles($root, "*.tmp")).Count -eq 0) "Temporary manifests leaked"
    Assert-True (@([System.IO.Directory]::GetFiles($root, "previous-*.json")).Count -eq 0) "Old manifest backups leaked"
    Write-Output "Windows manifest atomic publish and rollback tests passed"
}
finally {
    [System.IO.Directory]::Delete($root, $true)
}
