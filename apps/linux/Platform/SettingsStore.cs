using System.Text.Json;
using System.Text.Json.Serialization;
using PPVPN.App.Core.ViewModels;

namespace PPVPN.Linux.Platform;

/// <summary>App preferences in $XDG_CONFIG_HOME/ppvpn/settings.json.</summary>
public sealed class SettingsStore : ISettingsStore
{
    private static readonly JsonSerializerOptions Options = new()
    {
        WriteIndented = true,
        Converters = { new JsonStringEnumConverter() },
    };

    private readonly string _path;
    private readonly Dictionary<string, string> _placements = new();

    public SettingsStore(string? path = null)
    {
        _path = path ?? Path.Combine(LinuxPaths.ConfigDir, "settings.json");
        try
        {
            if (File.Exists(_path) && JsonSerializer.Deserialize<Stored>(File.ReadAllText(_path), Options) is { } stored)
            {
                Appearance = stored.Appearance;
                ApiBaseOverride = stored.ApiBaseOverride;
                BackgroundHintShown = stored.BackgroundHintShown;
                AutoCheckUpdates = stored.AutoCheckUpdates ?? true;
                foreach (var (window, placement) in stored.Placements ?? []) _placements[window] = placement;
            }
        }
        catch (JsonException)
        {
            // A corrupt file falls back to the defaults and is rewritten on the next save.
        }
    }

    public Appearance Appearance { get; set; }

    public string? ApiBaseOverride { get; set; }

    public bool BackgroundHintShown { get; set; }

    /// <summary>Not part of <see cref="ISettingsStore"/>: backs <see cref="LinuxAppServices.AutoCheckUpdates"/>.</summary>
    public bool AutoCheckUpdates { get; set; } = true;

    public string? GetWindowPlacement(string window) => _placements.GetValueOrDefault(window);

    public void SetWindowPlacement(string window, string? placement)
    {
        if (placement is null) _placements.Remove(window);
        else _placements[window] = placement;
    }

    public void Save()
    {
        Directory.CreateDirectory(Path.GetDirectoryName(_path)!);
        var staging = _path + ".new";
        var stored = new Stored(Appearance, ApiBaseOverride, BackgroundHintShown, AutoCheckUpdates, new(_placements));
        File.WriteAllText(staging, JsonSerializer.Serialize(stored, Options));
        File.Move(staging, _path, overwrite: true);
    }

    private sealed record Stored(
        Appearance Appearance,
        string? ApiBaseOverride,
        bool BackgroundHintShown,
        bool? AutoCheckUpdates,
        Dictionary<string, string>? Placements);
}
