# Installs the oncebitten release binaries on Windows.
#
#   irm https://raw.githubusercontent.com/bcmyguest/oncebitten/main/install.ps1 | iex
#
# ONCEBITTEN_VERSION      release to install, such as 0.1.0 (default: latest)
# ONCEBITTEN_INSTALL_DIR  where the binaries go (default: %LOCALAPPDATA%\oncebitten\bin)
# ONCEBITTEN_RELEASE_URL  releases base URL (default: the GitHub releases page)
$ErrorActionPreference = 'Stop'

$version = if ($env:ONCEBITTEN_VERSION) { $env:ONCEBITTEN_VERSION } else { 'latest' }
$installDir = if ($env:ONCEBITTEN_INSTALL_DIR) { $env:ONCEBITTEN_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'oncebitten\bin' }
$base = if ($env:ONCEBITTEN_RELEASE_URL) { $env:ONCEBITTEN_RELEASE_URL } else { 'https://github.com/bcmyguest/oncebitten/releases' }

if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') {
    throw "oncebitten install: no release build for Windows $env:PROCESSOR_ARCHITECTURE"
}
$name = 'oncebitten-x86_64-pc-windows-msvc.tar.gz'
$url = if ($version -eq 'latest') { "$base/latest/download/$name" } else { "$base/download/v$($version.TrimStart('v'))/$name" }

New-Item -ItemType Directory -Force -Path $installDir | Out-Null
$tmp = Join-Path $installDir ".oncebitten-install.$([guid]::NewGuid())"
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $archive = Join-Path $tmp $name
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $archive
    Invoke-WebRequest -UseBasicParsing -Uri "$url.sha256" -OutFile "$archive.sha256"
    $expected = ((Get-Content "$archive.sha256" -Raw) -split '\s+')[0]
    $actual = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLower()
    if ($actual -ne $expected.ToLower()) { throw "oncebitten install: checksum mismatch for $name" }
    $out = Join-Path $tmp 'out'
    New-Item -ItemType Directory -Path $out | Out-Null
    tar.exe -xzf $archive -C $out
    if ($LASTEXITCODE -ne 0) { throw "oncebitten install: cannot extract $name" }
    Get-ChildItem $out | Move-Item -Force -Destination $installDir
} finally {
    Remove-Item -Recurse -Force $tmp
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not (($userPath -split ';') -contains $installDir)) {
    [Environment]::SetEnvironmentVariable('Path', "$installDir;$userPath", 'User')
    Write-Host "Added $installDir to your user PATH; open a new terminal to use it."
}
Write-Host "oncebitten installed to $installDir"
