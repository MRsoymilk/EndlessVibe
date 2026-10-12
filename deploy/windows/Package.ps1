#requires -Version 5.1
<#
.SYNOPSIS
    Build one portable Windows EXE for distribution (no installer or sidecar DLLs).
.EXAMPLE
    .\deploy\windows\Package.ps1
.EXAMPLE
    .\deploy\windows\Package.ps1 -Offline
.DESCRIPTION
    Builds the MSVC x64 release with its C runtime linked statically and checks
    its PE import table. The only output in dist/windows is endlessvibe.exe.
    The EXE embeds its HTML/JS/CSS/icons and creates config/state per target user.
    Does NOT create or ship owner.key, tokens, config.toml or state.sqlite3.
    Requires an installed Rust x86_64-pc-windows-msvc target and MSVC build tools.
#>
[CmdletBinding()]
param(
    [string]$OutputPath,
    [switch]$Offline,
    [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
if ($env:OS -ne "Windows_NT") {
    throw "Windows portable EXE packages must be built on Windows with an MSVC toolchain."
}
$repo = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))
$manifest = Join-Path $repo "Cargo.toml"
$versionLine = Get-Content -LiteralPath $manifest | Select-String -Pattern '^\s*version\s*=\s*"([^"]+)"' | Select-Object -First 1
if ($null -eq $versionLine) {
    throw "Could not read the package version from Cargo.toml."
}
$version = $versionLine.Matches[0].Groups[1].Value
if ($version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.-]+)?$') {
    throw "Unexpected Cargo package version: $version"
}

function Read-UInt16At {
    param([System.IO.BinaryReader]$Reader, [long]$Offset)
    if ($Offset -lt 0 -or $Offset + 2 -gt $Reader.BaseStream.Length) {
        throw "PE header is truncated (UInt16 at $Offset)."
    }
    [void]$Reader.BaseStream.Seek($Offset, [System.IO.SeekOrigin]::Begin)
    return [uint16]$Reader.ReadUInt16()
}
function Read-UInt32At {
    param([System.IO.BinaryReader]$Reader, [long]$Offset)
    if ($Offset -lt 0 -or $Offset + 4 -gt $Reader.BaseStream.Length) {
        throw "PE header is truncated (UInt32 at $Offset)."
    }
    [void]$Reader.BaseStream.Seek($Offset, [System.IO.SeekOrigin]::Begin)
    return [uint32]$Reader.ReadUInt32()
}
function Convert-RvaToFileOffset {
    param([long]$Rva, [object[]]$Sections, [long]$Headers)
    if ($Rva -ge 0 -and $Rva -lt $Headers) { return $Rva }
    foreach ($section in $Sections) {
        $delta = $Rva - [long]$section.Rva
        if ($delta -ge 0 -and $delta -lt [long]$section.RawSize) {
            return ([long]$section.RawPointer + $delta)
        }
    }
    throw "PE RVA 0x$($Rva.ToString('X')) is not backed by file bytes."
}
function Read-AsciiZAt {
    param([System.IO.BinaryReader]$Reader, [long]$Offset)
    if ($Offset -lt 0 -or $Offset -ge $Reader.BaseStream.Length) {
        throw "Invalid PE DLL name offset: $Offset"
    }
    [void]$Reader.BaseStream.Seek($Offset, [System.IO.SeekOrigin]::Begin)
    $bytes = [System.Collections.Generic.List[byte]]::new()
    for ($i = 0; $i -lt 128; $i++) {
        if ($Reader.BaseStream.Position -ge $Reader.BaseStream.Length) { break }
        $value = $Reader.ReadByte()
        if ($value -eq 0) {
            if ($bytes.Count -eq 0) { throw "Empty PE import DLL name." }
            return [System.Text.Encoding]::ASCII.GetString($bytes.ToArray())
        }
        if ($value -lt 33 -or $value -gt 126) {
            throw "Invalid characters in PE DLL import name."
        }
        $bytes.Add($value)
    }
    throw "PE import DLL name is unterminated or too long."
}

function Get-PEImports {
    param([Parameter(Mandatory=$true)][string]$Path)
    $stream = [System.IO.File]::OpenRead($Path)
    $reader = [System.IO.BinaryReader]::new($stream)
    try {
        if ((Read-UInt16At $reader 0) -ne 0x5a4d) {
            throw "Built EXE is missing the DOS MZ signature."
        }
        $pe = [long](Read-UInt32At $reader 0x3c)
        if ($pe -lt 64 -or (Read-UInt32At $reader $pe) -ne 0x4550) {
            throw "Built EXE has no valid PE header."
        }
        if ((Read-UInt16At $reader ($pe + 4)) -ne 0x8664) {
            throw "Built EXE is not AMD64: use x86_64-pc-windows-msvc."
        }
        $sectionCount = [int](Read-UInt16At $reader ($pe + 6))
        $optionalSize = [int](Read-UInt16At $reader ($pe + 20))
        $optional = $pe + 24
        if ($sectionCount -lt 1 -or $sectionCount -gt 96 -or $optionalSize -lt 224) {
            throw "Invalid PE section table/optional header."
        }
        if ((Read-UInt16At $reader $optional) -ne 0x20b) {
            throw "Built EXE must use the PE32+ (64-bit) format."
        }
        $numDirs = Read-UInt32At $reader ($optional + 108)
        if ($numDirs -lt 14 -or $optionalSize -lt 224) {
            throw "PE data directories are incomplete."
        }
        $headers = [long](Read-UInt32At $reader ($optional + 60))
        $importRva = [long](Read-UInt32At $reader ($optional + 120))
        $importSize = [long](Read-UInt32At $reader ($optional + 124))
        $delayImportRva = [long](Read-UInt32At $reader ($optional + 216))
        $delayImportSize = [long](Read-UInt32At $reader ($optional + 220))
        if ($delayImportRva -ne 0 -or $delayImportSize -ne 0) {
            throw "Delay-load DLLs are not permitted in a single-file package."
        }
        if ($importRva -eq 0 -or $importSize -lt 20 -or $importSize -gt 1048576) {
            throw "PE import directory is missing or oversized."
        }
        $sections = @()
        $sectionBase = $optional + $optionalSize
        for ($i = 0; $i -lt $sectionCount; $i++) {
            $off = $sectionBase + 40 * $i
            $sections += [pscustomobject]@{
                Rva = [long](Read-UInt32At $reader ($off + 12))
                RawSize = [long](Read-UInt32At $reader ($off + 16))
                RawPointer = [long](Read-UInt32At $reader ($off + 20))
            }
        }
        $base = Convert-RvaToFileOffset -Rva $importRva -Sections $sections -Headers $headers
        $dlls = [System.Collections.Generic.List[string]]::new()
        $ended = $false
        for ($i = 0; $i -lt 256; $i++) {
            $offset = $base + 20 * $i
            $originalThunk = Read-UInt32At $reader $offset
            $nameRva = Read-UInt32At $reader ($offset + 12)
            $firstThunk = Read-UInt32At $reader ($offset + 16)
            if ($originalThunk -eq 0 -and $nameRva -eq 0 -and $firstThunk -eq 0) {
                $ended = $true
                break
            }
            if ($nameRva -eq 0) { throw "PE import DLL name RVA is invalid." }
            $nameOffset = Convert-RvaToFileOffset -Rva $nameRva -Sections $sections -Headers $headers
            $dlls.Add((Read-AsciiZAt -Reader $reader -Offset $nameOffset))
        }
        if (-not $ended) { throw "PE import descriptor list is unterminated." }
        return $dlls.ToArray()
    }
    finally {
        $reader.Dispose()
    }
}

# Only DLLs included with supported Windows 10/11 installations may be imported.
# The compiled EXE must not depend on VC++ redistributables or third-party DLLs.
$allowed = @(
    "KERNEL32.DLL", "KERNELBASE.DLL", "NTDLL.DLL", "ADVAPI32.DLL",
    "BCRYPT.DLL", "CRYPT32.DLL", "CRYPTBASE.DLL", "DNSAPI.DLL",
    "IPHLPAPI.DLL", "WS2_32.DLL", "WSOCK32.DLL", "MSWSOCK.DLL",
    "OLE32.DLL", "OLEAUT32.DLL", "USERENV.DLL", "USER32.DLL",
    "SHELL32.DLL", "SHLWAPI.DLL", "SECHOST.DLL", "SECUR32.DLL",
    "RPCRT4.DLL", "PSAPI.DLL", "POWRPROF.DLL", "VERSION.DLL",
    "GDI32.DLL", "GDI32FULL.DLL", "WINTRUST.DLL", "NCRYPT.DLL",
    "COMBASE.DLL", "NORMALIZ.DLL", "SETUPAPI.DLL", "NETAPI32.DLL",
    "WTSAPI32.DLL", "WINHTTP.DLL", "WEVTAPI.DLL"
)
$allowedNames = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
foreach ($item in $allowed) { [void]$allowedNames.Add($item) }

$target = "x86_64-pc-windows-msvc"
$targetDir = Join-Path $repo "target\package-windows"
$source = Join-Path $targetDir "$target\release\endlessvibe.exe"
if (-not $SkipBuild) {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw "Rust Cargo was not found. Install the stable MSVC toolchain."
    }
    $oldFlags = $env:RUSTFLAGS
    try {
        # Both Rust and bundled native crates (such as SQLite) must use the
        # static MSVC CRT; dynamic VCRUNTIME*.dll requires an extra installer.
        $env:RUSTFLAGS = (($oldFlags + " -C target-feature=+crt-static").Trim())
        $cargoArgs = @("build", "--locked", "--release", "--target", $target,
                       "--target-dir", $targetDir)
        if ($Offline) { $cargoArgs += "--offline" }
        Write-Host "Building EndlessVibe $version with the static MSVC CRT..."
        Push-Location $repo
        try {
            & cargo @cargoArgs
            if ($LASTEXITCODE -ne 0) {
                throw "Cargo build failed (exit $LASTEXITCODE). Fetch dependencies on a trusted Windows build machine."
            }
        }
        finally {
            Pop-Location
        }
    }
    finally {
        if ($null -eq $oldFlags) {
            Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue
        }
        else { $env:RUSTFLAGS = $oldFlags }
    }
}
if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
    throw "The release executable is missing: $source"
}

$reported = (& $source --version | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $reported -ne "EndlessVibe $version") {
    throw "Built EXE failed version smoke test: $reported (expected EndlessVibe $version)."
}
& $source --help | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Built EXE failed --help smoke test." }

$imports = @(Get-PEImports -Path $source)
if ($imports.Count -eq 0) { throw "No PE imports could be audited." }
foreach ($name in $imports) {
    $dll = $name.ToUpperInvariant()
    if ($dll -match '^(API-MS-WIN-|EXT-MS-WIN-)[A-Z0-9-]+\.DLL$') { continue }
    if (-not $allowedNames.Contains($dll)) {
        throw "EXE depends on non-allowlisted DLL '$name'. Refusing single-file release. Audit whether it is shipped with Windows; do not bundle secrets or unknown DLLs."
    }
}
Write-Host ("Verified PE imports: " + (($imports | Sort-Object -Unique) -join ", "))

if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $repo "dist\windows\endlessvibe.exe"
}
elseif (-not [System.IO.Path]::IsPathRooted($OutputPath)) {
    $OutputPath = Join-Path $repo $OutputPath
}
$destination = [System.IO.Path]::GetFullPath($OutputPath)
if ([System.IO.Path]::GetExtension($destination) -ine ".exe") {
    throw "OutputPath must name one .exe file."
}
$destDir = Split-Path -Parent $destination
[System.IO.Directory]::CreateDirectory($destDir) | Out-Null
if ([string]::Equals($destination, [System.IO.Path]::GetFullPath($source),
        [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "The destination cannot be the Cargo build output."
}

Copy-Item -LiteralPath $source -Destination $destination -Force
$sourceHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
$releaseHash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash
if ($sourceHash -ine $releaseHash) {
    throw "Copied EXE has a different checksum; the release may be incomplete."
}
Write-Host ""
Write-Host "Single-file package: $destination"
Write-Host "Version: EndlessVibe $version"
Write-Host "SHA256: $releaseHash"
Write-Host "No config.toml, owner.key, SQLite database or extra DLL was packaged."
Write-Host ("Usage: & '" + $destination.Replace("'", "''") + "' --init")
