param(
    [switch]$NoClean
)

. (Join-Path $PSScriptRoot "windows-package.ps1")

$stageDir = New-YTuffWindowsStage -Clean:(-not $NoClean)
$zipPath = New-YTuffWindowsZip -StageDir $stageDir

Write-Host "`n=== Build Complete ===" -ForegroundColor Cyan
Write-Host "Release folder: $stageDir" -ForegroundColor Yellow
Write-Host "Release zip: $zipPath" -ForegroundColor Yellow
