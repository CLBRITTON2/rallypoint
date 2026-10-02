<#
.SYNOPSIS
Runs the unit tests in src, which need no desktop.
#>
. (Join-Path $PSScriptRoot 'cargo.ps1')

Invoke-Cargo @('test', '--manifest-path', $manifest, '--lib', '--bins')
