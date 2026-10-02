<#
.SYNOPSIS
Builds the release exe and publishes it as the GitHub release for an existing tag, with the tag's RELEASENOTES.md
section as its notes.

.PARAMETER Tag
The pushed tag, `v` then the version in Cargo.toml, as v0.1.0.

.PARAMETER Token
A GitHub token that can write releases: the workflow's GITHUB_TOKEN, or `gh auth token` locally.

.EXAMPLE
.\scripts\release.ps1 -Tag v0.1.0 -Token (gh auth token)
#>
param(
    [Parameter(Mandatory)][string]$Tag,
    [Parameter(Mandatory)][string]$Token
)
. (Join-Path $PSScriptRoot 'cargo.ps1')
$root = Resolve-Path (Join-Path $PSScriptRoot '..')

$version = (cargo metadata --manifest-path $manifest --no-deps --format-version 1 | ConvertFrom-Json).packages |
    Where-Object name -EQ 'rallypoint' |
    Select-Object -ExpandProperty version
if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed with exit code $LASTEXITCODE"
}
if ($Tag -ne "v$version") {
    throw "tag $Tag does not match version $version in $manifest"
}
$notes = & (Join-Path $PSScriptRoot 'extract-version-release-notes.ps1') -Version $version

Invoke-Cargo @('build', '--manifest-path', $manifest, '--release')

$staging = Join-Path $root "target\release-package\rallypoint-$Tag"
$zip = Join-Path $root "target\rallypoint-$Tag-x86_64-pc-windows-msvc.zip"
if (Test-Path $staging) {
    Remove-Item -Recurse $staging
}
New-Item -ItemType Directory $staging | Out-Null
$files = 'target\release\rallypoint.exe', 'LICENSE', 'README.md'
Copy-Item -Path ($files | ForEach-Object { Join-Path $root $_ }) -Destination $staging
Compress-Archive -Path (Join-Path $staging '*') -DestinationPath $zip -Force
$notesFile = Join-Path $root "target\release-package\notes-$Tag.md"
Set-Content -Path $notesFile -Value $notes

$env:GH_TOKEN = $Token
& gh release create $Tag $zip --repo CLBRITTON2/rallypoint --title $Tag --notes-file $notesFile --verify-tag
if ($LASTEXITCODE -ne 0) {
    throw "gh release create $Tag failed with exit code $LASTEXITCODE"
}
