using System.Security.Cryptography;

namespace BluetoothTransfer.Core.Server;

/// <summary>文件 SHA-256 计算（Core 内自包含，避免依赖 GUI 工程）。</summary>
public static class AsstHash
{
    public static async Task<string> ComputeSha256Async(string path, CancellationToken ct = default)
    {
        using var sha = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        using var fs = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
        var buffer = new byte[81920];
        int read;
        while ((read = await fs.ReadAsync(buffer, ct)) > 0)
            sha.AppendData(buffer, 0, read);
        return Convert.ToHexString(sha.GetHashAndReset());
    }
}
