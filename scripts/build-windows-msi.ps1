param(
    [switch]$NoClean
)

. (Join-Path $PSScriptRoot "windows-package.ps1")

$stageDir = New-YTuffWindowsStage -Clean:(-not $NoClean)
$msiPath = New-YTuffWindowsMsi -StageDir $stageDir
$productCode = Get-MsiProperty -Path $msiPath -Property "ProductCode"
$upgradeCode = Get-MsiProperty -Path $msiPath -Property "UpgradeCode"

Write-Host "`n=== MSI Build Complete ===" -ForegroundColor Cyan
Write-Host "Release folder: $stageDir" -ForegroundColor Yellow
Write-Host "Installer: $msiPath" -ForegroundColor Yellow
Write-Host "ProductCode: $productCode" -ForegroundColor Yellow
Write-Host "UpgradeCode: $upgradeCode" -ForegroundColor Yellow
