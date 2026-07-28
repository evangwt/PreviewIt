$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$workflowPath = Join-Path $root '.github\workflows\foundation.yml'
$workflow = [System.IO.File]::ReadAllText($workflowPath)
$gatePath = Join-Path $root 'tools\test-foundation.ps1'
$gate = [System.IO.File]::ReadAllText($gatePath)

$foundationJob = [regex]::Match(
    $workflow,
    '(?ms)^  foundation:\s*\r?\n(?<body>.*?)(?=^  [A-Za-z0-9_-]+:\s*(?:\r?\n|\z)|\z)'
)
if (-not $foundationJob.Success) {
    throw 'Foundation job not found in .github/workflows/foundation.yml'
}

if (-not [regex]::IsMatch($foundationJob.Groups['body'].Value, '(?m)^    runs-on: windows-2022\r?$')) {
    throw 'Foundation job must use runs-on: windows-2022'
}

if (-not [regex]::IsMatch($foundationJob.Groups['body'].Value, '(?m)^          "WIX=\$wixRoot\\" \| Out-File -FilePath \$env:GITHUB_ENV -Encoding utf8 -Append\r?$')) {
    throw 'Foundation WiX root must end with a backslash'
}

if (-not [regex]::IsMatch($foundationJob.Groups['body'].Value, '(?m)^          "WixToolPath=\$wixBin\\" \| Out-File -FilePath \$env:GITHUB_ENV -Encoding utf8 -Append\r?$')) {
    throw 'Foundation WiX tool path must end with a backslash'
}

if (-not [regex]::IsMatch($gate, '(?m)^Invoke-Checked ''workflow-runner'' \{ pwsh -NoProfile -File tests/baseline/foundation-workflow\.tests\.ps1 \}\r?$')) {
    throw 'Foundation gate must run workflow runner contract'
}

Write-Output 'FOUNDATION_WORKFLOW_OK'
