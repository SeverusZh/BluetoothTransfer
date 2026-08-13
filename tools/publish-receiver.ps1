param(
    [string]$OutputDir = "artifacts/btrecv",
    [switch]$Zip
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$proj = Join-Path $root "src/BluetoothTransfer.Receiver/BluetoothTransfer.Receiver.csproj"
$sc = Join-Path $OutputDir "selfcontained"
$fd = Join-Path $OutputDir "frameworkdependent"
$cli = Join-Path $OutputDir "cli-only-selfcontained"
$cliFd = Join-Path $OutputDir "cli-only-frameworkdependent"

dotnet publish $proj -c Release -r win-x64 --self-contained true  -p:PublishSingleFile=true -o $sc
if ($LASTEXITCODE -ne 0) { Write-Error "接收端自包含发布失败（退出码 $LASTEXITCODE）"; exit 1 }
dotnet publish $proj -c Release -r win-x64 --self-contained false -p:PublishSingleFile=true -o $fd
if ($LASTEXITCODE -ne 0) { Write-Error "接收端框架依赖发布失败（退出码 $LASTEXITCODE）"; exit 1 }
dotnet publish $proj -c Release -r win-x64 --self-contained true -p:PublishSingleFile=true -p:PublishCliOnly=true -p:PublishTrimmed=true -o $cli
if ($LASTEXITCODE -ne 0) { Write-Error "纯 CLI 自包含发布失败（退出码 $LASTEXITCODE）"; exit 1 }
dotnet publish $proj -c Release -r win-x64 --self-contained false -p:PublishSingleFile=true -p:PublishCliOnly=true -o $cliFd
if ($LASTEXITCODE -ne 0) { Write-Error "纯 CLI 框架依赖发布失败（退出码 $LASTEXITCODE）"; exit 1 }

Write-Host "已生成："
Write-Host "  自包含（开箱即用）：$sc\btrecv.exe"
Write-Host "  框架依赖（需 .NET 8 运行时）：$fd\btrecv.exe"
Write-Host "  纯 CLI 自包含（裁剪，最小引导）：$cli\btrecv.exe"
Write-Host "  纯 CLI 框架依赖（需 .NET 8 运行时）：$cliFd\btrecv.exe"

if ($Zip) {
    try {
        Compress-Archive -Path (Join-Path $sc "*") -DestinationPath (Join-Path $OutputDir "btrecv-selfcontained.zip") -Force
        if (-not $?) { Write-Error "接收端自包含压缩包生成失败"; exit 1 }
        Compress-Archive -Path (Join-Path $cli "*") -DestinationPath (Join-Path $OutputDir "btrecv-cli-only.zip") -Force
        if (-not $?) { Write-Error "接收端纯 CLI 压缩包生成失败"; exit 1 }
    } catch {
        Write-Error "接收端压缩包生成失败：$_"
        exit 1
    }
    Write-Host "  压缩包：$OutputDir\btrecv-selfcontained.zip / btrecv-cli-only.zip"
}
