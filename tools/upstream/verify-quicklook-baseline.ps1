$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$required = @(
    'src\legacy\quicklook\QuickLook.sln',
    'src\legacy\quicklook\LICENSE-GPL.txt',
    'UPSTREAM.md'
)

foreach ($path in $required) {
    if (-not (Test-Path -LiteralPath (Join-Path $root $path))) {
        throw "Missing required upstream artifact: $path"
    }
}

$provenance = Get-Content -Raw (Join-Path $root 'UPSTREAM.md')
$expected = 'b13df028f3cce1f84792f7043b57bf5cea3a3e4c'
if (-not $provenance.Contains($expected)) {
    throw "UPSTREAM.md does not pin $expected"
}

Write-Output "QUICKLOOK_BASELINE_OK=$expected"
