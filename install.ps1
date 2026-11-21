# Install a signed Meld release on Windows (meld, meld-lsp, meld-pack,
# Lang Pack data, licenses and docs). Missing language runtimes and cosign are
# installed by default via winget; pass -SkipDependencies to opt out.
param(
    [string]$Version = 'v0.2.0',
    [string]$Target = 'x86_64-pc-windows-msvc',
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Meld'),
    [switch]$SkipSignatureVerification,
    [switch]$SkipDependencies,
    [string]$Repository = 'enis1enis2/meld'
)

$ErrorActionPreference = 'Stop'

function Test-CommandOnPath {
    param([string[]]$Names)
    foreach ($name in $Names) {
        if (Get-Command $name -ErrorAction SilentlyContinue) { return $true }
    }
    return $false
}

function Install-WingetDependency {
    param(
        [string]$Label,
        [string[]]$Commands,
        [string]$PackageId
    )
    if (Test-CommandOnPath $Commands) {
        Write-Host "install.ps1: $Label already available"
        return
    }
    if ($SkipDependencies) {
        Write-Warning "install.ps1: $Label missing; install $PackageId manually"
        return
    }
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
        Write-Warning "install.ps1: $Label missing and winget is unavailable; install $PackageId manually"
        return
    }
    Write-Host "install.ps1: installing $Label via winget ($PackageId)"
    & winget install --id $PackageId --exact --silent --accept-package-agreements --accept-source-agreements
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "install.ps1: winget could not install $PackageId (exit $LASTEXITCODE); install manually"
    }
}

$tag = $Version
if (-not $tag.StartsWith('v')) { $tag = "v$tag" }
$plainVersion = $tag.TrimStart('v')
# Anchor the Sigstore identity to this exact tag; no broad refs/tags/.* regex.
$identity = '^https://github\.com/{0}/\.github/workflows/release\.yml@refs/tags/{1}$' -f `
    ([regex]::Escape($Repository)), ([regex]::Escape($tag))
$base = "https://github.com/$Repository/releases/download/$tag"
$archiveName = "meld-lang-$plainVersion-$Target.zip"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("meld-install-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null

try {
    Install-WingetDependency -Label 'Python 3' -Commands @('python3', 'py', 'python') -PackageId 'Python.Python.3.12'
    Install-WingetDependency -Label 'Node.js' -Commands @('node') -PackageId 'OpenJS.NodeJS.LTS'
    Install-WingetDependency -Label 'Lua' -Commands @('lua') -PackageId 'DEVCOM.Lua'
    # Git for Windows also provides sh.exe for the bundled Bash pack.
    Install-WingetDependency -Label 'Git (provides sh)' -Commands @('git', 'sh') -PackageId 'Git.Git'
    Install-WingetDependency -Label 'cosign' -Commands @('cosign') -PackageId 'Sigstore.Cosign'

    # winget installs update the persisted PATH but not this already-running
    # shell. Reload it so the checks and Meld wrappers below see new tools.
    $machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($machinePath -and $userPath) { $env:Path = "$machinePath;$userPath" }
    elseif ($machinePath) { $env:Path = $machinePath }
    elseif ($userPath) { $env:Path = $userPath }

    $archive = Join-Path $tmp $archiveName
    $sums = Join-Path $tmp 'SHA256SUMS'
    $archivePattern = [regex]::Escape($archiveName)
    Write-Host "install.ps1: downloading $archiveName"
    Invoke-WebRequest -Uri "$base/$archiveName" -OutFile $archive -UseBasicParsing
    Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile $sums -UseBasicParsing

    $expected = (Get-Content $sums | Where-Object { $_ -match "  $archivePattern$" } | Select-Object -First 1) -split '  ' | Select-Object -First 1
    if (-not $expected) { throw "$archiveName is not listed in SHA256SUMS" }
    $actual = (Get-FileHash -Algorithm SHA256 -Path $archive).Hash.ToLowerInvariant()
    if ($actual -ne $expected.ToLowerInvariant()) {
        throw "checksum mismatch: expected $expected got $actual"
    }
    Write-Host 'install.ps1: PASS SHA256SUMS verification'

    $cosign = Get-Command cosign -ErrorAction SilentlyContinue
    if ($cosign -and -not $SkipSignatureVerification) {
        Invoke-WebRequest -Uri "$base/$archiveName.bundle" -OutFile "$archive.bundle" -UseBasicParsing
        & cosign verify-blob --bundle "$archive.bundle" `
            --certificate-identity-regexp $identity `
            --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' `
            $archive
        if ($LASTEXITCODE -ne 0) { throw 'cosign signature verification failed' }
        Write-Host 'install.ps1: PASS Sigstore signature verification'
    } else {
        Write-Host 'install.ps1: signature verification skipped (integrity checksum only)'
    }

    $extract = Join-Path $tmp 'extract'
    Expand-Archive -Path $archive -DestinationPath $extract -Force
    $payload = Get-ChildItem -Directory -Path $extract | Select-Object -First 1
    if (-not $payload) { throw 'archive has no payload directory' }

    $realBin = Join-Path $InstallDir 'lib\meld-lang\bin'
    $share = Join-Path $InstallDir 'share\meld-lang'
    $packs = Join-Path $share 'packs'
    $doc = Join-Path $InstallDir 'share\doc\meld-lang'
    $wrapperBin = Join-Path $InstallDir 'bin'
    foreach ($dir in @($realBin, $share, $packs, $doc, $wrapperBin)) {
        if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }
    Copy-Item -Path (Join-Path $payload.FullName 'bin\*') -Destination $realBin -Force
    Copy-Item -Path (Join-Path $payload.FullName 'packs\*') -Destination $packs -Recurse -Force
    Copy-Item -Path (Join-Path $payload.FullName 'share\doc\meld-lang\*') -Destination $doc -Recurse -Force

    # MELD_PACKS is colon-separated in the core; Windows drive letters contain
    # a colon, so the wrappers set MELD_ROOT (the pack root under share/) and
    # leave MELD_PACKS for explicitly added third-party pack roots.
    foreach ($name in @('meld', 'meld-lsp', 'meld-pack')) {
        $target = Join-Path $realBin "$name.exe"
        $wrapper = Join-Path $wrapperBin "$name.cmd"
        $body = @"
@echo off
set "MELD_ROOT=$share"
if defined MELD_PACKS set "MELD_PACKS=%MELD_PACKS%"
"$target" %*
"@
        Set-Content -Path $wrapper -Value $body -Encoding ASCII
    }

    Write-Host "install.ps1: installed Meld $tag to $InstallDir"
    Write-Host "install.ps1: add $wrapperBin to PATH"
    Write-Host 'install.ps1: open a new terminal if newly installed runtimes are not on PATH yet'
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
