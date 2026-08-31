#Requires -Version 5.1

<#
.SYNOPSIS
    Installs the latest tmdbtag release on Windows.
.DESCRIPTION
    Downloads the architecture-specific GitHub Release archive, verifies its
    SHA-256 checksum, installs the binary for the current user, and updates
    the user's PATH.
#>

$ErrorActionPreference = "Stop"

$DefaultRepo = "Theryston/tmdbtag"
if ([string]::IsNullOrWhiteSpace($env:TMDBTAG_REPOSITORY)) {
    $Repo = $DefaultRepo
} else {
    $Repo = $env:TMDBTAG_REPOSITORY
}

$BinaryName = "tmdbtag"
if ([string]::IsNullOrWhiteSpace($env:TMDBTAG_INSTALL_DIR)) {
    $InstallDir = Join-Path $env:LOCALAPPDATA "Programs\tmdbtag"
} else {
    $InstallDir = $env:TMDBTAG_INSTALL_DIR
}

Write-Host ""
Write-Host "  tmdbtag installer" -ForegroundColor Cyan
Write-Host "  Installing the latest release..." -ForegroundColor Cyan
Write-Host ""

function Get-TargetArchitecture {
    $NativeArchitecture = $env:PROCESSOR_ARCHITEW6432
    if ([string]::IsNullOrWhiteSpace($NativeArchitecture)) {
        $NativeArchitecture = $env:PROCESSOR_ARCHITECTURE
    }

    switch ($NativeArchitecture) {
        "AMD64" { return "x86_64" }
        "ARM64" { return "aarch64" }
        "x86" {
            if ([Environment]::Is64BitOperatingSystem) {
                return "x86_64"
            }
            return "unknown"
        }
        default { return "unknown" }
    }
}

$Architecture = Get-TargetArchitecture
if ($Architecture -eq "unknown") {
    Write-Host "Error: unsupported architecture." -ForegroundColor Red
    Write-Host "This installer supports x86_64 and ARM64."
    exit 1
}

$Target = "$Architecture-pc-windows-msvc"
Write-Host "Detected target: " -NoNewline -ForegroundColor Yellow
Write-Host $Target

$TempDir = $null
try {
    Write-Host "Fetching the latest release..." -ForegroundColor Cyan
    $Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -UseBasicParsing
    $Version = [string]$Release.tag_name

    if ([string]::IsNullOrWhiteSpace($Version) -or $Version -notmatch '^v[0-9]+[A-Za-z0-9.+-]*$') {
        throw "The latest release did not contain a valid version tag."
    }

    $ArchiveName = "$BinaryName-$Target.zip"
    $DownloadUrl = "https://github.com/$Repo/releases/download/$Version/$ArchiveName"
    $ChecksumUrl = "https://github.com/$Repo/releases/download/$Version/checksums-sha256.txt"

    Write-Host "Latest version: " -NoNewline -ForegroundColor Green
    Write-Host $Version
    Write-Host "Downloading $ArchiveName..." -ForegroundColor Cyan

    $TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $TempDir -Force | Out-Null

    $ArchivePath = Join-Path $TempDir $ArchiveName
    $ChecksumPath = Join-Path $TempDir "checksums-sha256.txt"

    Invoke-WebRequest -Uri $DownloadUrl -OutFile $ArchivePath -UseBasicParsing
    Invoke-WebRequest -Uri $ChecksumUrl -OutFile $ChecksumPath -UseBasicParsing

    $ExpectedChecksum = $null
    foreach ($Line in Get-Content -LiteralPath $ChecksumPath) {
        $Parts = $Line -split "\s+", 2
        if ($Parts.Count -eq 2 -and $Parts[1].TrimStart([char]"*") -eq $ArchiveName) {
            $ExpectedChecksum = $Parts[0].ToLowerInvariant()
            break
        }
    }

    if ([string]::IsNullOrWhiteSpace($ExpectedChecksum)) {
        throw "The release checksum is missing."
    }

    $ActualChecksum = (Get-FileHash -Algorithm SHA256 -LiteralPath $ArchivePath).Hash.ToLowerInvariant()
    if ($ActualChecksum -ne $ExpectedChecksum) {
        throw "Checksum verification failed."
    }

    Write-Host "Extracting..." -ForegroundColor Cyan
    Expand-Archive -LiteralPath $ArchivePath -DestinationPath $TempDir -Force

    $BinaryPath = Join-Path $TempDir "$BinaryName.exe"
    if (-not (Test-Path -LiteralPath $BinaryPath -PathType Leaf)) {
        throw "The release archive has an unexpected layout."
    }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $DestinationPath = Join-Path $InstallDir "$BinaryName.exe"
    $StagedPath = Join-Path $InstallDir "$BinaryName.new.exe"

    Write-Host "Installing to: " -NoNewline -ForegroundColor Cyan
    Write-Host $InstallDir
    Copy-Item -LiteralPath $BinaryPath -Destination $StagedPath -Force
    Move-Item -LiteralPath $StagedPath -Destination $DestinationPath -Force

    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $PathEntries = @()
    if (-not [string]::IsNullOrWhiteSpace($UserPath)) {
        $PathEntries = @($UserPath -split ";")
    }

    if ($PathEntries -notcontains $InstallDir) {
        $PathEntries += $InstallDir
        $NewUserPath = $PathEntries -join ";"
        [Environment]::SetEnvironmentVariable("Path", $NewUserPath, "User")
    }

    if ($env:Path -notlike "*$InstallDir*") {
        $env:Path = "$env:Path;$InstallDir"
    }

    Write-Host ""
    Write-Host "tmdbtag installed successfully." -ForegroundColor Green
    Write-Host "Run 'tmdbtag --help' to get started." -ForegroundColor Yellow
    Write-Host "Restart your terminal if the PATH change is not immediately visible." -ForegroundColor DarkYellow
} catch {
    Write-Host ""
    Write-Host "Installation failed: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
} finally {
    if ($TempDir -and (Test-Path -LiteralPath $TempDir)) {
        Remove-Item -LiteralPath $TempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
