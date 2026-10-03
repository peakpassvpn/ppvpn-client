<#
.SYNOPSIS
  Generates the notification-area icons (apps/windows/PPVPN.Windows/Assets/Tray/*.ico).

.DESCRIPTION
  Monochrome states from the brand mark, the same scheme as the macOS menu bar icons: the
  glyph is always solid (an outline is unreadable at 16 px) and only its opacity changes.
  Dimmer states are 60 %, not macOS's 45 %: next to the heavier Windows tray glyphs 45 %
  looked washed out.
    on    = 100 %
    busy1 = 80 %, busy2 = 60 % (2-frame breathing, like macOS alternating busy and off)
    off   = 60 %
    error = 60 % with a solid dot in the bottom-right corner
  in two sets: light-*.ico (dark glyph, light taskbar) and dark-*.ico (white glyph, dark taskbar).
  Each .ico holds 16, 20, 24, 32, 40, 48 and 64 px 32-bit images (BMP entries, which
  System.Drawing.Icon and every shell version read).

  Runs on Windows PowerShell 5.1 (System.Drawing + Add-Type); no downloads.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File apps\windows\scripts\make-tray-icons.ps1
#>
[CmdletBinding()]
param(
  # Brand mark with a transparent or white background (256 px or larger works best).
  [string] $Source,
  [string] $OutDir,
  # Keep preview-*.png (16 and 64 px) next to the icons for review.
  [switch] $KeepPreviews
)

$ErrorActionPreference = "Stop"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
if (-not $Source) { $Source = Join-Path $repo "assets\icons\128x128@2x.png" }
if (-not $OutDir) { $OutDir = Join-Path $repo "apps\windows\PPVPN.Windows\Assets\Tray" }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @"
using System;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.IO;
using System.Collections.Generic;

public static class TrayIcons
{
    const int N = 256; // working resolution

    // Coverage of the mark: alpha, with near-white pixels treated as background.
    static float[] Mask(string path)
    {
        using (var source = new Bitmap(path))
        using (var work = new Bitmap(N, N, PixelFormat.Format32bppArgb))
        {
            using (var g = Graphics.FromImage(work))
            {
                g.Clear(Color.Transparent);
                g.InterpolationMode = InterpolationMode.HighQualityBicubic;
                // Small inset so the stroke never touches the icon edge.
                g.DrawImage(source, new Rectangle(6, 6, N - 12, N - 12));
            }
            var mask = new float[N * N];
            for (int y = 0; y < N; y++)
                for (int x = 0; x < N; x++)
                {
                    var c = work.GetPixel(x, y);
                    float a = c.A / 255f;
                    float white = Math.Min(c.R, Math.Min(c.G, c.B)) / 255f;
                    float ink = white > 0.85f ? 0f : 1f;
                    mask[y * N + x] = a * ink;
                }
            return mask;
        }
    }

    static Bitmap Render(float[] mask, string shape, int size, Color color)
    {
        float opacity = shape == "on" ? 1f : shape == "busy1" ? 0.8f : 0.6f;

        using (var big = new Bitmap(N, N, PixelFormat.Format32bppArgb))
        {
            for (int y = 0; y < N; y++)
                for (int x = 0; x < N; x++)
                {
                    int i = y * N + x;
                    float a = mask[i];
                    big.SetPixel(x, y, Color.FromArgb((int)(Math.Min(1f, a) * 255 * opacity), color));
                }
            var result = new Bitmap(size, size, PixelFormat.Format32bppArgb);
            using (var g = Graphics.FromImage(result))
            {
                g.Clear(Color.Transparent);
                g.InterpolationMode = InterpolationMode.HighQualityBicubic;
                g.PixelOffsetMode = PixelOffsetMode.HighQuality;
                g.DrawImage(big, new Rectangle(0, 0, size, size));
                if (shape == "error")
                {
                    // Badge: a full-opacity dot bottom-right, with a transparent ring cut into the
                    // glyph around it so it reads as separate at 16 px.
                    float r = Math.Max(3f, size * 0.2f);
                    float ring = Math.Max(1f, size / 16f);
                    float cx = size - r, cy = size - r;
                    g.SmoothingMode = SmoothingMode.AntiAlias;
                    g.CompositingMode = CompositingMode.SourceCopy;
                    using (var clear = new SolidBrush(Color.Transparent))
                        g.FillEllipse(clear, cx - r - ring, cy - r - ring, 2 * (r + ring), 2 * (r + ring));
                    g.CompositingMode = CompositingMode.SourceOver;
                    using (var ink = new SolidBrush(color))
                        g.FillEllipse(ink, cx - r, cy - r, 2 * r, 2 * r);
                }
            }
            return result;
        }
    }

    static byte[] Dib(Bitmap bmp)
    {
        int s = bmp.Width;
        using (var ms = new MemoryStream())
        using (var w = new BinaryWriter(ms))
        {
            w.Write(40); w.Write(s); w.Write(s * 2); w.Write((short)1); w.Write((short)32);
            w.Write(0); w.Write(0); w.Write(0); w.Write(0); w.Write(0); w.Write(0);
            for (int y = s - 1; y >= 0; y--)
                for (int x = 0; x < s; x++)
                {
                    var c = bmp.GetPixel(x, y);
                    w.Write(c.B); w.Write(c.G); w.Write(c.R); w.Write(c.A);
                }
            int maskRow = ((s + 31) / 32) * 4;
            w.Write(new byte[maskRow * s]);
            return ms.ToArray();
        }
    }

    public static void Write(string source, string outDir)
    {
        var mask = Mask(source);
        int[] sizes = { 16, 20, 24, 32, 40, 48, 64 };
        var sets = new Dictionary<string, Color> { { "light", Color.FromArgb(0x1C, 0x1C, 0x1C) }, { "dark", Color.White } };
        foreach (var set in sets)
            foreach (var shape in new[] { "off", "busy1", "busy2", "on", "error" })
            {
                var images = new List<byte[]>();
                foreach (var size in sizes)
                    using (var bmp = Render(mask, shape, size, set.Value))
                    {
                        images.Add(Dib(bmp));
                        if (size == 64 || size == 16) bmp.Save(Path.Combine(outDir, "preview-" + set.Key + "-" + shape + "-" + size + ".png"), ImageFormat.Png);
                    }
                var path = Path.Combine(outDir, set.Key + "-" + shape + ".ico");
                using (var f = File.Create(path))
                using (var w = new BinaryWriter(f))
                {
                    w.Write((short)0); w.Write((short)1); w.Write((short)sizes.Length);
                    int offset = 6 + 16 * sizes.Length;
                    for (int k = 0; k < sizes.Length; k++)
                    {
                        w.Write((byte)(sizes[k] >= 256 ? 0 : sizes[k])); w.Write((byte)(sizes[k] >= 256 ? 0 : sizes[k]));
                        w.Write((byte)0); w.Write((byte)0); w.Write((short)1); w.Write((short)32);
                        w.Write(images[k].Length); w.Write(offset);
                        offset += images[k].Length;
                    }
                    foreach (var image in images) w.Write(image);
                }
            }
    }
}
"@

[TrayIcons]::Write((Resolve-Path $Source).Path, (Resolve-Path $OutDir).Path)
# Previews are for review only; keep them out of the app.
if (-not $KeepPreviews) { Get-ChildItem $OutDir -Filter preview-*.png | Remove-Item }
Get-ChildItem $OutDir -Filter *.ico | ForEach-Object { "{0,-18} {1,7} bytes" -f $_.Name, $_.Length }
