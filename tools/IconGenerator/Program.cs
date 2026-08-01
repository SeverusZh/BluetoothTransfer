using System.IO;
using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Markup;

// 图标生成工具：读取 src/BluetoothTransfer/Assets/AppIcons.xaml 中的 AppIcon DrawingImage，
// 按多尺寸渲染生成 Assets/app.png 与 Assets/app.ico（PNG-in-ICO，Vista+ 支持多尺寸）。
// 用法：dotnet run --project tools/IconGenerator [assets目录]

var assetsDir = args.Length > 0 ? Path.GetFullPath(args[0]) : LocateAssetsDir();
var xamlPath = Path.Combine(assetsDir, "AppIcons.xaml");
if (!File.Exists(xamlPath))
{
    Console.Error.WriteLine($"未找到 {xamlPath}");
    return 2;
}

var dictionary = (ResourceDictionary)XamlReader.Parse(File.ReadAllText(xamlPath));
if (dictionary["AppIcon"] is not DrawingImage drawingImage)
{
    Console.Error.WriteLine("AppIcons.xaml 中缺少 AppIcon 资源");
    return 2;
}

var sizes = new[] { 16, 24, 32, 48, 64, 128, 256 };
var pngs = new Dictionary<int, byte[]>();
foreach (var size in sizes)
{
    pngs[size] = RenderPng(drawingImage, size);
}

var png256Path = Path.Combine(assetsDir, "app.png");
File.WriteAllBytes(png256Path, pngs[256]);

var icoPath = Path.Combine(assetsDir, "app.ico");
File.WriteAllBytes(icoPath, BuildIco(sizes, pngs));

Console.WriteLine($"已生成：{png256Path}、{icoPath}（尺寸 {string.Join("/", sizes)}）");
return 0;

static byte[] RenderPng(DrawingImage source, int size)
{
    var visual = new DrawingVisual();
    using (var dc = visual.RenderOpen())
    {
        dc.PushTransform(new ScaleTransform(size / 24.0, size / 24.0));
        dc.DrawDrawing(source.Drawing);
        dc.Pop();
    }
    var bitmap = new RenderTargetBitmap(size, size, 96, 96, PixelFormats.Pbgra32);
    bitmap.Render(visual);
    var encoder = new PngBitmapEncoder();
    encoder.Frames.Add(BitmapFrame.Create(bitmap));
    using var ms = new MemoryStream();
    encoder.Save(ms);
    return ms.ToArray();
}

static byte[] BuildIco(int[] sizes, IReadOnlyDictionary<int, byte[]> pngs)
{
    using var ms = new MemoryStream();
    using var writer = new BinaryWriter(ms);
    var count = sizes.Length;
    writer.Write((ushort)0);           // reserved
    writer.Write((ushort)1);           // type: icon
    writer.Write((ushort)count);

    var offset = 6 + 16 * count;
    foreach (var size in sizes)
    {
        var data = pngs[size];
        writer.Write((byte)(size >= 256 ? 0 : size)); // 宽（0=256）
        writer.Write((byte)(size >= 256 ? 0 : size)); // 高
        writer.Write((byte)0);                        // 调色板
        writer.Write((byte)0);                        // 保留
        writer.Write((ushort)1);                      // 色彩平面
        writer.Write((ushort)32);                     // 位深
        writer.Write(data.Length);                    // 数据长度
        writer.Write(offset);                         // 数据偏移
        offset += data.Length;
    }
    foreach (var size in sizes)
    {
        writer.Write(pngs[size]);
    }
    writer.Flush();
    return ms.ToArray();
}

static string LocateAssetsDir()
{
    var current = Directory.GetCurrentDirectory();
    var root = FindRepoRoot(current) ?? FindRepoRoot(AppContext.BaseDirectory)
        ?? throw new DirectoryNotFoundException("无法定位仓库根目录（BluetoothTransfer.sln）");
    return Path.Combine(root, "src", "BluetoothTransfer", "Assets");
}

static string? FindRepoRoot(string start)
{
    var dir = new DirectoryInfo(start);
    while (dir != null)
    {
        if (File.Exists(Path.Combine(dir.FullName, "BluetoothTransfer.sln")))
            return dir.FullName;
        dir = dir.Parent;
    }
    return null;
}
