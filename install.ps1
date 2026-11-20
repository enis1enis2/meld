# Install a signed Meld release on Windows (meld, meld-lsp, meld-pack,
# Lang Pack data, licenses and docs). No language runtime is installed here.
param(
    [string]$Version = 'v0.2.0-rc.1',
    [string]$Target = 'x86_64-pc-windows-msvc',
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Meld'),
    [switch]$SkipSignatureVerification,
    [string]$Repository = 'enis1enis2/meld'
)

$ErrorActionPreference = 'Stop'
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
    $doc = Join-Path $InstallDir 'share\doc\meld-lang'
    $wrapperBin = Join-Path $InstallDir 'bin'
    foreach ($dir in @($realBin, $share, $doc, $wrapperBin)) {
        if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }
    Copy-Item -Path (Join-Path $payload.FullName 'bin\*') -Destination $realBin -Force
    Copy-Item -Path (Join-Path $payload.FullName 'packs\*') -Destination (Join-Path $share 'packs') -Recurse -Force
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
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
