param(
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Release'
)

$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
$solution = Join-Path $root 'src\legacy\quicklook\QuickLook.sln'
$msbuild = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" `
    -latest -products * -requires Microsoft.Component.MSBuild `
    -find 'MSBuild\**\Bin\MSBuild.exe' | Select-Object -First 1

if (-not $msbuild) { throw 'Visual Studio MSBuild was not found' }

$realGit = (Get-Command git.exe -ErrorAction Stop).Source
$shimDirectory = Join-Path ([IO.Path]::GetTempPath()) `
    ("previewit-quicklook-git-{0}" -f [Guid]::NewGuid().ToString('N'))
$shimPath = Join-Path $shimDirectory 'git.cmd'
$originalPath = $env:PATH
$originalWix = $env:WIX
$wixRoot = $env:WIX
if (-not $wixRoot) {
    $wixRoot = [Environment]::GetEnvironmentVariable('WIX', 'Machine')
}
if (-not $wixRoot -or -not (Test-Path -LiteralPath (Join-Path $wixRoot 'bin\heat.exe'))) {
    throw 'WiX Toolset v3.14 build tools were not found'
}

New-Item -ItemType Directory -Path $shimDirectory | Out-Null
$shim = @"
@echo off
if /I "%~1"=="describe" (
  echo 4.5.0
  exit /b 0
)
"$realGit" %*
"@
[IO.File]::WriteAllText($shimPath, $shim, [Text.Encoding]::ASCII)

try {
    $env:PATH = "$shimDirectory;$originalPath"
    $env:WIX = $wixRoot
    & $msbuild $solution /m:1 /nr:false /restore `
        "/p:Configuration=$Configuration" '/p:Platform=Any CPU' /v:minimal
    $buildExitCode = $LASTEXITCODE
}
finally {
    $env:PATH = $originalPath
    $env:WIX = $originalWix
    Remove-Item -LiteralPath $shimPath -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $shimDirectory -Force -ErrorAction SilentlyContinue
}

if ($buildExitCode -ne 0) { exit $buildExitCode }
