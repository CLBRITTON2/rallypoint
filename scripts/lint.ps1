<#
.SYNOPSIS
Fails on unformatted code or any clippy warning.
#>
. (Join-Path $PSScriptRoot 'cargo.ps1')

Invoke-Cargo @('fmt', '--manifest-path', $manifest, '--check')
Invoke-Cargo @('clippy', '--manifest-path', $manifest, '--all-targets', '--', '-D', 'warnings')
