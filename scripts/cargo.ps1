# Dot-sourced by the CI scripts.
$ErrorActionPreference = 'Stop'
$manifest = Join-Path $PSScriptRoot '..\Cargo.toml'

function Invoke-Cargo([string[]]$arguments) {
    & cargo @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}
