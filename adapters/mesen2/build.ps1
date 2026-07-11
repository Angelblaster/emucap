# Build the emucap-compatible MesenCE host from a pinned upstream revision.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$Here = Split-Path -Parent $MyInvocation.MyCommand.Path
$LockFile = Join-Path $Here "upstream.lock"
$DefaultWork = Join-Path $Here "work"
$Work = if ($env:EMUCAP_MESEN_WORK) { $env:EMUCAP_MESEN_WORK } else { $DefaultWork }

function Get-LockValue([string]$Name) {
    $line = Get-Content -LiteralPath $LockFile | Where-Object { $_ -like "$Name=*" } | Select-Object -First 1
    if (-not $line) { throw "missing $Name in $LockFile" }
    return $line.Substring($Name.Length + 1)
}

function Invoke-Native([string]$Command, [string[]]$Arguments) {
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Command failed with exit code $LASTEXITCODE"
    }
}

$MesenRepo = Get-LockValue "MESEN_REPO"
$MesenTag = Get-LockValue "MESEN_TAG"
$MesenCommit = Get-LockValue "MESEN_COMMIT"
$MesenHostApi = [int](Get-LockValue "MESEN_HOST_API")
$MesenPatchsetHash = Get-LockValue "MESEN_PATCHSET_SHA256"
if ($MesenPatchsetHash -notmatch '^[0-9a-f]{64}$') {
    throw "invalid MESEN_PATCHSET_SHA256 in $LockFile"
}

New-Item -ItemType Directory -Force -Path $Work | Out-Null
$WorkItem = Get-Item -Force -LiteralPath $Work
if (($WorkItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "Mesen work path must not be a reparse point: $Work"
}
$Work = $WorkItem.FullName
$Marker = Join-Path $Work ".emucap-mesen-work"
if (-not (Test-Path -LiteralPath $Marker)) {
    $entries = @(Get-ChildItem -Force -LiteralPath $Work)
    if ($env:EMUCAP_MESEN_WORK -and $entries.Count -gt 0) {
        throw "EMUCAP_MESEN_WORK is not empty or emucap-owned: $Work"
    }
    New-Item -ItemType File -Path $Marker | Out-Null
}

$LockDir = Join-Path $Work ".build.lock"
$OwnerFile = Join-Path $LockDir "owner.json"
while ($true) {
    try {
        New-Item -ItemType Directory -Path $LockDir -ErrorAction Stop | Out-Null
        break
    } catch {
        $owner = $null
        try { $owner = Get-Content -Raw -LiteralPath $OwnerFile | ConvertFrom-Json } catch {}
        $live = $null
        if ($owner -and $owner.pid) {
            try { $live = Get-Process -Id ([int]$owner.pid) -ErrorAction Stop } catch {}
        }
        $liveStart = if ($live) { $live.StartTime.ToUniversalTime().Ticks } else { $null }
        if (-not $owner -or -not $live -or $liveStart -ne [long]$owner.start_ticks) {
            Write-Warning "Reclaiming dead Mesen build lock: $LockDir"
            Remove-Item -Force -LiteralPath $OwnerFile -ErrorAction SilentlyContinue
            Remove-Item -Force -LiteralPath $LockDir -ErrorAction SilentlyContinue
            continue
        }
        Write-Host "Waiting for Mesen build owned by pid $($owner.pid)"
        Start-Sleep -Seconds 1
    }
}

@{
    pid = $PID
    start_ticks = (Get-Process -Id $PID).StartTime.ToUniversalTime().Ticks
} | ConvertTo-Json | Set-Content -Encoding UTF8 -LiteralPath $OwnerFile

try {
    $Source = Join-Path $Work "mesen"
    if (Test-Path -LiteralPath $Source) {
        $SourceItem = Get-Item -Force -LiteralPath $Source
        if (($SourceItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Mesen work source must not be a reparse point: $Source"
        }
    }
    $Origin = if ($env:EMUCAP_MESEN_SRC) { $env:EMUCAP_MESEN_SRC } else { $MesenRepo }
    if ($env:EMUCAP_MESEN_SRC -and -not (Test-Path -LiteralPath (Join-Path $Origin ".git"))) {
        throw "EMUCAP_MESEN_SRC is not a git checkout: $Origin"
    }
    if (-not (Test-Path -LiteralPath (Join-Path $Source ".git"))) {
        if ((Test-Path -LiteralPath $Source) -and @(Get-ChildItem -Force -LiteralPath $Source).Count -gt 0) {
            throw "Mesen work source exists but is not a git checkout: $Source"
        }
        New-Item -ItemType Directory -Force -Path $Source | Out-Null
        Invoke-Native "git" @("init", "-q", $Source)
        Invoke-Native "git" @("-C", $Source, "remote", "add", "origin", $Origin)
    } else {
        Invoke-Native "git" @("-C", $Source, "remote", "set-url", "origin", $Origin)
    }

    Write-Host "Fetching MesenCE $MesenTag ($MesenCommit)"
    Invoke-Native "git" @("-C", $Source, "fetch", "-q", "--depth", "1", "origin", $MesenCommit)
    Invoke-Native "git" @("-C", $Source, "checkout", "-q", "--detach", $MesenCommit)
    $Got = (& git -C $Source rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0 -or $Got -ne $MesenCommit) {
        throw "Mesen revision mismatch: got $Got expected $MesenCommit"
    }
    Invoke-Native "git" @("-C", $Source, "reset", "-q", "--hard", $MesenCommit)
    Invoke-Native "git" @("-C", $Source, "clean", "-fdq")
    if (Test-Path -LiteralPath (Join-Path $Source ".gitmodules")) {
        Invoke-Native "git" @("-C", $Source, "submodule", "update", "--init", "--recursive")
    }

    $Patches = @(
        (Join-Path $Here "patches/0001-fix-numeric-cli-settings.patch"),
        (Join-Path $Here "patches/0002-add-code-break-idle-event.patch")
    )
    $patchStream = [System.IO.MemoryStream]::new()
    try {
        foreach ($Patch in $Patches) {
            if (-not (Test-Path -LiteralPath $Patch)) { throw "missing Mesen patch: $Patch" }
            $bytes = [System.IO.File]::ReadAllBytes($Patch)
            $patchStream.Write($bytes, 0, $bytes.Length)
        }
        $patchBytes = $patchStream.ToArray()
    } finally {
        $patchStream.Dispose()
    }
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $ActualPatchsetHash = -join ($sha.ComputeHash($patchBytes) | ForEach-Object { $_.ToString("x2") })
    } finally {
        $sha.Dispose()
    }
    if ($ActualPatchsetHash -ne $MesenPatchsetHash) {
        throw "Mesen patch stack does not match upstream.lock: expected=$MesenPatchsetHash actual=$ActualPatchsetHash"
    }
    foreach ($Patch in $Patches) {
        Write-Host "Applying $(Split-Path -Leaf $Patch)"
        Invoke-Native "git" @("-C", $Source, "apply", "--check", $Patch)
        Invoke-Native "git" @("-C", $Source, "apply", $Patch)
    }

    $VsWhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio/Installer/vswhere.exe"
    $MsBuild = $null
    if (Test-Path -LiteralPath $VsWhere) {
        $MsBuild = (& $VsWhere -latest -requires Microsoft.Component.MSBuild -find "MSBuild\**\Bin\MSBuild.exe" | Select-Object -First 1)
    }
    if (-not $MsBuild) {
        $command = Get-Command msbuild -ErrorAction SilentlyContinue
        if ($command) { $MsBuild = $command.Source }
    }
    if (-not $MsBuild) { throw "Visual Studio 2022 MSBuild was not found" }

    Write-Host "Building MesenCE locally with Visual Studio Release/x64"
    Get-ChildItem -Recurse -File -Filter "emucap-mesen-build.json" -Path (Join-Path $Source "bin") -ErrorAction SilentlyContinue |
        Remove-Item -Force -ErrorAction SilentlyContinue
    Invoke-Native $MsBuild @((Join-Path $Source "Mesen.sln"), "/m", "/p:Configuration=Release", "/p:Platform=x64")
    $Binary = Get-ChildItem -Recurse -File -Filter "Mesen.exe" -Path (Join-Path $Source "bin/win-x64/Release") |
        Select-Object -First 1
    if (-not $Binary) { throw "Mesen build completed without Mesen.exe under bin/win-x64/Release" }

    $Metadata = Join-Path $Binary.DirectoryName "emucap-mesen-build.json"
    @{
        upstream = $MesenRepo
        tag = $MesenTag
        commit = $MesenCommit
        host_api = $MesenHostApi
        patchset_sha256 = $MesenPatchsetHash
    } | ConvertTo-Json | Set-Content -Encoding UTF8 -LiteralPath $Metadata

    Write-Host "OK: $($Binary.FullName)"
    Write-Host "metadata: $Metadata"
} finally {
    Remove-Item -Force -LiteralPath $OwnerFile -ErrorAction SilentlyContinue
    Remove-Item -Force -LiteralPath $LockDir -ErrorAction SilentlyContinue
}
