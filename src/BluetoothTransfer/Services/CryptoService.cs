using System.Security.Cryptography;

namespace BluetoothTransfer.Services;

public class CryptoService
{
    private ECDiffieHellman? _ecdh;
    private byte[]? _sessionKey;

    public byte[] GetPublicKey()
    {
        _ecdh = ECDiffieHellman.Create(ECCurve.NamedCurves.nistP256);
        return _ecdh.PublicKey.ExportSubjectPublicKeyInfo();
    }

    public void DeriveSessionKey(byte[] peerPublicKey)
    {
        if (_ecdh == null) throw new InvalidOperationException("Call GetPublicKey first");
        using var peerKey = ECDiffieHellman.Create();
        peerKey.ImportSubjectPublicKeyInfo(peerPublicKey, out _);
        _sessionKey = _ecdh.DeriveKeyMaterial(peerKey.PublicKey);
    }

    public byte[] Encrypt(byte[] plaintext)
    {
        if (_sessionKey == null) throw new InvalidOperationException("No session key");
        var key = new byte[32];
        Array.Copy(_sessionKey, key, 32);

        var nonce = new byte[12];
        RandomNumberGenerator.Fill(nonce);
        var ciphertext = new byte[plaintext.Length];
        var tag = new byte[16];

        using var aes = new AesGcm(key, 16);
        aes.Encrypt(nonce, plaintext, ciphertext, tag);

        var result = new byte[12 + ciphertext.Length + 16];
        Array.Copy(nonce, 0, result, 0, 12);
        Array.Copy(ciphertext, 0, result, 12, ciphertext.Length);
        Array.Copy(tag, 0, result, 12 + ciphertext.Length, 16);
        return result;
    }

    public byte[] Decrypt(byte[] data)
    {
        if (_sessionKey == null) throw new InvalidOperationException("No session key");
        var key = new byte[32];
        Array.Copy(_sessionKey, key, 32);

        var nonce = new byte[12];
        Array.Copy(data, 0, nonce, 0, 12);

        var ciphertextLen = data.Length - 12 - 16;
        var ciphertext = new byte[ciphertextLen];
        Array.Copy(data, 12, ciphertext, 0, ciphertextLen);

        var tag = new byte[16];
        Array.Copy(data, 12 + ciphertextLen, tag, 0, 16);

        var plaintext = new byte[ciphertextLen];
        using var aes = new AesGcm(key, 16);
        aes.Decrypt(nonce, ciphertext, tag, plaintext);
        return plaintext;
    }

    public bool HasSessionKey => _sessionKey != null;
}
