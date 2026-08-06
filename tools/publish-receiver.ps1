param(
    [string]$OutputDir = "artifacts/btrecv",
    [switch]$Zip
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$proj = Join-Path $root "src/BluetoothTransfer.Receiver/BluetoothTransfer.Receiver.csproj"
$sc = Join-Path $OutputDir "selfcontained"
$fd = Join-Path $OutputDir "frameworkdependent"

dotnet publish $proj -c Release -r win-x64 --self-contained true  -p:PublishSingleFile=true -o $sc
dotnet publish $proj -c Release -r win-x64 --self-contained false -p:PublishSingleFile=true -o $fd

Write-Host "已生成："
Write-Host "  自包含（开箱即用）：$sc\btrecv.exe"
Write-Host "  框架依赖（需 .NET 8 运行时）：$fd\btrecv.exe"

if ($Zip) {
    $zipPath = Join-Path $OutputDir "btrecv-selfcontained.zip"
    Compress-Archive -Path (Join-Path $sc "*") -DestinationPath $zipPath -Force
    Write-Host "  压缩包：$zipPath"
}
