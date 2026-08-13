param(
    [string]$OutputDir = "artifacts/gui",
    [switch]$Zip
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$proj = Join-Path $root "src/BluetoothTransfer/BluetoothTransfer.csproj"
$sc = Join-Path $OutputDir "selfcontained"
$fd = Join-Path $OutputDir "frameworkdependent"

dotnet publish $proj -c Release -r win-x64 --self-contained true  -p:PublishSingleFile=true -o $sc
if ($LASTEXITCODE -ne 0) { Write-Error "GUI 自包含发布失败（退出码 $LASTEXITCODE）"; exit 1 }
dotnet publish $proj -c Release -r win-x64 --self-contained false -p:PublishSingleFile=true -o $fd
if ($LASTEXITCODE -ne 0) { Write-Error "GUI 框架依赖发布失败（退出码 $LASTEXITCODE）"; exit 1 }

Write-Host "已生成："
Write-Host "  完整 GUI 自包含（开箱即用）：$sc\BluetoothTransfer.exe"
Write-Host "  完整 GUI 框架依赖（需 .NET 8 桌面运行时）：$fd\BluetoothTransfer.exe"

if ($Zip) {
    try {
        Compress-Archive -Path (Join-Path $sc "*") -DestinationPath (Join-Path $OutputDir "BluetoothTransfer-selfcontained.zip") -Force
        if (-not $?) { Write-Error "GUI 自包含压缩包生成失败"; exit 1 }
    } catch {
        Write-Error "GUI 自包含压缩包生成失败：$_"
        exit 1
    }
    Write-Host "  压缩包：$OutputDir\BluetoothTransfer-selfcontained.zip"
}
