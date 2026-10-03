<#
.SYNOPSIS
Runs the unit tests in src and the CLI tests in tests, which need no desktop.
#>
. (Join-Path $PSScriptRoot 'cargo.ps1')

Invoke-Cargo @('test', '--manifest-path', $manifest, '--all-targets')
