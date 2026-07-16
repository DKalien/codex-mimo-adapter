using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;

namespace CodexMiMoLauncher;

/// <summary>Stores the key outside the repository with the current Windows user's DPAPI profile.</summary>
internal sealed class SecretStore
{
    private readonly string _path;

    public SecretStore(string directory) => _path = Path.Combine(directory, "mimo-api-key.dpapi");

    public string? Load()
    {
        if (!File.Exists(_path)) return null;
        var protectedBytes = File.ReadAllBytes(_path);
        var plainBytes = Dpapi.Unprotect(protectedBytes);
        try { return Encoding.UTF8.GetString(plainBytes); }
        finally { CryptographicOperations.ZeroMemory(plainBytes); }
    }

    public void Save(string value)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(_path)!);
        var plaintext = Encoding.UTF8.GetBytes(value);
        try { File.WriteAllBytes(_path, Dpapi.Protect(plaintext)); }
        finally { CryptographicOperations.ZeroMemory(plaintext); }
    }

    public void Clear()
    {
        if (File.Exists(_path)) File.Delete(_path);
    }
}

internal static class Dpapi
{
    private const int CryptProtectUiForbidden = 0x1;

    public static byte[] Protect(byte[] data) => Transform(data, protect: true);
    public static byte[] Unprotect(byte[] data) => Transform(data, protect: false);

    private static byte[] Transform(byte[] data, bool protect)
    {
        var input = new DataBlob(data);
        try
        {
            Blob output;
            var success = protect
                ? CryptProtectData(ref input.Blob, null, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero, CryptProtectUiForbidden, out output)
                : CryptUnprotectData(ref input.Blob, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero, CryptProtectUiForbidden, out output);
            if (!success) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "Windows DPAPI operation failed");
            try
            {
                var result = new byte[output.cbData];
                if (output.cbData > 0) Marshal.Copy(output.pbData, result, 0, output.cbData);
                return result;
            }
            finally
            {
                if (output.pbData != IntPtr.Zero) LocalFree(output.pbData);
            }
        }
        finally { input.Dispose(); }
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct Blob { public int cbData; public IntPtr pbData; }

    private sealed class DataBlob : IDisposable
    {
        public Blob Blob;
        public DataBlob(byte[] bytes)
        {
            Blob.cbData = bytes.Length;
            Blob.pbData = Marshal.AllocHGlobal(bytes.Length);
            if (bytes.Length > 0) Marshal.Copy(bytes, 0, Blob.pbData, bytes.Length);
        }
        public void Dispose()
        {
            if (Blob.pbData == IntPtr.Zero) return;
            if (Blob.cbData > 0)
            {
                var empty = new byte[Blob.cbData];
                Marshal.Copy(empty, 0, Blob.pbData, Blob.cbData);
            }
            Marshal.FreeHGlobal(Blob.pbData);
            Blob.pbData = IntPtr.Zero;
        }
    }

    [DllImport("crypt32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    private static extern bool CryptProtectData(ref Blob input, string? description, IntPtr optionalEntropy, IntPtr reserved, IntPtr promptStruct, int flags, out Blob output);
    [DllImport("crypt32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    private static extern bool CryptUnprotectData(ref Blob input, IntPtr description, IntPtr optionalEntropy, IntPtr reserved, IntPtr promptStruct, int flags, out Blob output);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr LocalFree(IntPtr memory);
}
