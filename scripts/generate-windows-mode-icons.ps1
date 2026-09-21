param(
    [string]$OutputDirectory = '',
    [string]$PreviewPath = ''
)

$ErrorActionPreference = 'Stop'
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path (Split-Path -Parent $PSScriptRoot) 'crates\keytao-windows-ime'
}
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
Add-Type -AssemblyName System.Drawing

# Render font outlines, not a ClearType screenshot: alpha must remain transparent
# and every visible pixel must stay white at all Windows tray/DPI sizes.
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.IO;

public static class KeyTaoModeIcons
{
    private static readonly int[] Sizes = { 16, 20, 24, 32, 40, 48, 64 };

    private static Bitmap Render(string glyph, int size)
    {
        var bitmap = new Bitmap(size, size, PixelFormat.Format32bppArgb);
        using (var family = new FontFamily("Microsoft YaHei UI"))
        using (var path = new GraphicsPath())
        using (var format = (StringFormat)StringFormat.GenericTypographic.Clone())
        using (var graphics = Graphics.FromImage(bitmap))
        {
            path.AddString(glyph, family, (int)FontStyle.Regular, 100, PointF.Empty, format);
            RectangleF bounds = path.GetBounds();
            float extent = size * 0.875f; // 14px at 100% DPI; no colored tile/padding.
            float scale = extent / Math.Max(bounds.Width, bounds.Height);
            float x = (size - bounds.Width * scale) / 2 - bounds.X * scale;
            float y = (size - bounds.Height * scale) / 2 - bounds.Y * scale;
            using (var transform = new Matrix(scale, 0, 0, scale, x, y))
                path.Transform(transform);
            graphics.Clear(Color.Transparent);
            graphics.SmoothingMode = SmoothingMode.AntiAlias;
            graphics.CompositingMode = CompositingMode.SourceCopy;
            graphics.FillPath(Brushes.White, path);
        }
        return bitmap;
    }

    private static byte[] Frame(Bitmap bitmap)
    {
        int size = bitmap.Width;
        int maskStride = ((size + 31) / 32) * 4;
        using (var stream = new MemoryStream())
        using (var writer = new BinaryWriter(stream))
        {
            // 32-bit ICO DIB: bottom-up BGRA bitmap followed by a 1-bit AND mask.
            writer.Write((uint)40);
            writer.Write(size);
            writer.Write(size * 2);
            writer.Write((ushort)1);
            writer.Write((ushort)32);
            writer.Write((uint)0);
            writer.Write((uint)(size * size * 4 + maskStride * size));
            writer.Write(0); writer.Write(0); writer.Write(0); writer.Write(0);
            for (int y = size - 1; y >= 0; y--)
                for (int x = 0; x < size; x++)
                {
                    byte alpha = bitmap.GetPixel(x, y).A;
                    writer.Write((byte)255); writer.Write((byte)255);
                    writer.Write((byte)255); writer.Write(alpha);
                }
            for (int y = size - 1; y >= 0; y--)
            {
                var mask = new byte[maskStride];
                for (int x = 0; x < size; x++)
                    if (bitmap.GetPixel(x, y).A == 0)
                        mask[x / 8] |= (byte)(0x80 >> (x % 8));
                writer.Write(mask);
            }
            return stream.ToArray();
        }
    }

    private static void WriteIcon(string path, string glyph)
    {
        var frames = new List<byte[]>();
        foreach (int size in Sizes)
            using (var bitmap = Render(glyph, size))
                frames.Add(Frame(bitmap));
        using (var stream = File.Create(path))
        using (var writer = new BinaryWriter(stream))
        {
            writer.Write((ushort)0); writer.Write((ushort)1);
            writer.Write((ushort)Sizes.Length);
            uint offset = (uint)(6 + 16 * Sizes.Length);
            for (int i = 0; i < Sizes.Length; i++)
            {
                writer.Write((byte)Sizes[i]); writer.Write((byte)Sizes[i]);
                writer.Write((ushort)0);
                writer.Write((ushort)1); writer.Write((ushort)32);
                writer.Write((uint)frames[i].Length); writer.Write(offset);
                offset += (uint)frames[i].Length;
            }
            foreach (byte[] frame in frames) writer.Write(frame);
        }
    }

    public static void Generate(string directory, string preview)
    {
        WriteIcon(Path.Combine(directory, "mode-zh.ico"), "\u4e2d");
        WriteIcon(Path.Combine(directory, "mode-en.ico"), "\u82f1");
        if (String.IsNullOrEmpty(preview)) return;
        Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(preview)));
        using (var bitmap = new Bitmap(400, 120))
        using (var graphics = Graphics.FromImage(bitmap))
        {
            graphics.Clear(Color.FromArgb(36, 36, 36));
            int x = 20;
            foreach (int size in new[] { 16, 20, 24, 32, 40, 48 })
            {
                using (var zh = Render("\u4e2d", size)) graphics.DrawImageUnscaled(zh, x, 12);
                using (var en = Render("\u82f1", size)) graphics.DrawImageUnscaled(en, x, 68);
                x += size + 25;
            }
            bitmap.Save(preview, ImageFormat.Png);
        }
    }
}
'@

[KeyTaoModeIcons]::Generate($OutputDirectory, $PreviewPath)
Write-Host "Generated transparent white mode icons in $OutputDirectory"
