using System.IO;
using BluetoothTransfer.Core.Server;

namespace BluetoothTransfer.Services;

/// <summary>
/// 接收文件落盘路径解析（纯文件系统逻辑，无 UI/存储依赖，便于单元测试）。
///
/// 背景：<see cref="AsstFileSink"/> 完成时会用 <c>ResolveFinalName</c> 处理重名——无冲突写 base 名，
/// 冲突依次改为 <c>stem (1).ext</c>、<c>stem (2).ext</c>…（如 <c>recv.bin</c> → <c>recv (1).bin</c>）；但 AssistantServer 的完成回调只带 <c>AsstHello</c>，
/// 不暴露最终路径。因此这里按候选名扫描落盘目录，并用「文件大小 == hello.FileSize」做轻量校验，
/// 取第一个匹配的文件：
/// 1. 不重新计算 SHA-256（避免接收完成路径对大文件做二次哈希，拖慢完成回调）；
/// 2. 一个候选都匹配不上时返回空串，让复制规划器提示「没有本地文件路径」，而不是复制到同名的旧文件。
/// 取舍：若目录中恰好存在同名且大小完全相同的旧文件，仍可能误判（极端情形，接受不重算 SHA 的代价）。
/// </summary>
internal static class ReceiveLocalPathResolver
{
    /// <summary>重名后缀扫描上限：AsstFileSink 从 (1) 开始递增，正常场景远小于 20。</summary>
    internal const int MaxCollisionSuffix = 20;

    /// <summary>
    /// 解析接收落盘文件的绝对路径；<paramref name="saveDir"/> 为空或找不到大小匹配的文件时返回空串。
    /// </summary>
    /// <param name="saveDir">接收保存目录（StartAsync 时已转为绝对路径）。</param>
    /// <param name="fileName">协议 hello 中的原始文件名（可能与落盘名不同，需按 AsstFileSink 规则规范化）。</param>
    /// <param name="fileSize">本次接收的文件大小，用于区分重名旧文件。</param>
    internal static string Resolve(string saveDir, string fileName, long fileSize)
    {
        if (string.IsNullOrWhiteSpace(saveDir)) return "";
        var baseName = AsstFileSink.Sanitize(fileName);
        var extension = Path.GetExtension(baseName);
        var stem = Path.GetFileNameWithoutExtension(baseName);
        for (var suffix = 0; suffix <= MaxCollisionSuffix; suffix++)
        {
            var candidateName = suffix == 0 ? baseName : $"{stem} ({suffix}){extension}";
            var candidate = Path.Combine(saveDir, candidateName);
            try
            {
                var info = new FileInfo(candidate);
                if (info.Exists && info.Length == fileSize) return info.FullName;
            }
            catch (IOException)
            {
                // 候选文件在扫描过程中被移动/占用：跳过该候选，继续尝试下一个。
            }
            catch (UnauthorizedAccessException)
            {
                // 无权限读取候选文件：跳过，继续尝试下一个。
            }
        }
        return "";
    }
}
