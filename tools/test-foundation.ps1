$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $root

function Invoke-Checked {
    param(
        [Parameter(Mandatory)]
        [string]$Name,
        [Parameter(Mandatory)]
        [scriptblock]$Command
    )

    Write-Output "FOUNDATION_STEP=$Name"
    & $Command
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}

Invoke-Checked 'quicklook-provenance' { pwsh -NoProfile -File tools/upstream/verify-quicklook-baseline.ps1 }
Invoke-Checked 'legacy-build' { pwsh -NoProfile -File tests/baseline/legacy-build.tests.ps1 }
Invoke-Checked 'rustfmt' { cargo fmt --manifest-path src/rust/Cargo.toml --all -- --check }
Invoke-Checked 'clippy' { cargo clippy --manifest-path src/rust/Cargo.toml --workspace --all-targets -- -D warnings }

# The Rust integration suites execute the Release x64 worker directly. A clean
# checkout therefore has to build it before `cargo test --workspace`.
Invoke-Checked 'worker-build' { dotnet build src/dotnet/PreviewIt.WorkerProbe/PreviewIt.WorkerProbe.csproj -c Release }
Invoke-Checked 'broker-build' { cargo build --manifest-path src/rust/Cargo.toml -p previewit-broker }
Invoke-Checked 'broker-single-instance' { cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test broker_single_instance -- --test-threads=1 }
Invoke-Checked 'rust-tests' { cargo test --manifest-path src/rust/Cargo.toml --workspace }
Invoke-Checked 'dotnet-tests' { dotnet test tests/dotnet/PreviewIt.Protocol.Tests/PreviewIt.Protocol.Tests.csproj -c Release }

Write-Output 'FOUNDATION_GATE_OK'
