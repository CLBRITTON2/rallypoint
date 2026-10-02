<#
.SYNOPSIS
Returns the release notes for one version, read out of RELEASENOTES.md.

.DESCRIPTION
The notes are the lines between that version's `## [<Version>] - <date>` heading and the next `## ` heading.
release.ps1 calls this and publishes the result as the GitHub release body. Throws when the version has no section
or an empty one, so a release never goes out without notes.

.PARAMETER Version
The version as in Cargo.toml, as 0.1.0.

.EXAMPLE
.\scripts\extract-version-release-notes.ps1 -Version 0.1.0
#>
param(
    [Parameter(Mandatory)][string]$Version
)
$ErrorActionPreference = 'Stop'
$releaseNotes = Join-Path $PSScriptRoot '..\RELEASENOTES.md'

$lines = Get-Content $releaseNotes
$heading = "## [$Version]"
$start = [array]::FindIndex([string[]]$lines, [Predicate[string]] { param($line) $line.StartsWith($heading) })
if ($start -lt 0) {
    throw "$releaseNotes has no '$heading' section"
}
$end = [array]::FindIndex([string[]]$lines, $start + 1, [Predicate[string]] { param($line) $line.StartsWith('## ') })
$section = if ($end -lt 0) { $lines[($start + 1)..($lines.Count - 1)] } else { $lines[($start + 1)..($end - 1)] }
$notes = ($section -join "`n").Trim()
if (-not $notes) {
    throw "the '$heading' section of $releaseNotes is empty"
}
$notes
