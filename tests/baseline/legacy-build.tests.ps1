$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
& (Join-Path $root 'tools\build-legacy.ps1') -Configuration Release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$expected = Join-Path $root 'src\legacy\quicklook\Build'
if (-not (Test-Path -LiteralPath $expected)) {
    throw "Legacy build did not create $expected"
}
Write-Output 'LEGACY_BUILD_OK'
