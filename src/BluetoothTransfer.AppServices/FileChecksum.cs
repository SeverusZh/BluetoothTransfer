using System.IO;
using System.Security.Cryptography;

namespace BluetoothTransfer.Services;

/// <summary>文件 SHA-256 校验和助手（供记录留档使用，接收端为系统协议无法回传校验）。</summary>
public static class FileChecksum
{
    public static async Task<string> ComputeSha256Async(string path)
    {
        using var sha = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        using var fs = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
        var buffer = new byte[81920];
        int read;
        while ((read = await fs.ReadAsync(buffer)) > 0)
            sha.AppendData(buffer, 0, read);
        return Convert.ToHexString(sha.GetHashAndReset());
    }
}
