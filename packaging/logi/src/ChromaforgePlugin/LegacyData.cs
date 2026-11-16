// Carries the plugin's data over from its name before the rename (LightCraft).
// Copyright (c) 2026 RAD Films - Roman Alurkoff. MIT License.

namespace Chromaforge.Logi;

/// <summary>
/// This plugin was called LightCraft before. Its pairing token (<c>token.txt</c>) and its
/// <c>config.json</c> (the port) lived in the Logi data folder <c>PluginData\LightCraft</c>, next to
/// this plugin's own <c>PluginData\Chromaforge</c>. <see cref="Migrate"/> copies any of them this
/// plugin doesn't have yet, so an existing pairing keeps working without approving the plugin again.
/// The old folder is left as it is.
/// </summary>
internal static class LegacyData
{
    /// <summary>The old plugin's data folder name, a sibling of this plugin's.</summary>
    public const string OldName = "LightCraft";

    /// <summary>The files carried over.</summary>
    public static readonly string[] Files = { "token.txt", "config.json" };

    private const long MaxBytes = 1 << 20;

    /// <summary>Copy what's missing from the old data folder into <paramref name="dataDir"/>; returns
    /// the names copied. Never throws.</summary>
    public static List<string> Migrate(string dataDir, Action<string>? log)
    {
        var copied = new List<string>();
        try
        {
            var parent = Path.GetDirectoryName(dataDir);
            if (string.IsNullOrEmpty(parent)) return copied;
            var old = Path.Combine(parent, OldName);
            if (!Directory.Exists(old) || string.Equals(Path.GetFullPath(old), Path.GetFullPath(dataDir), StringComparison.OrdinalIgnoreCase)) return copied;
            foreach (var name in Files)
            {
                var from = Path.Combine(old, name);
                var to = Path.Combine(dataDir, name);
                if (!File.Exists(from) || File.Exists(to) || new FileInfo(from).Length > MaxBytes) continue;
                Directory.CreateDirectory(dataDir);
                File.Copy(from, to);
                copied.Add(name);
            }
            if (copied.Count > 0) log?.Invoke($"carried over from the LightCraft plugin: {string.Join(", ", copied)}");
        }
        catch (Exception e)
        {
            log?.Invoke($"carrying over the LightCraft plugin's pairing: {e.Message}");
        }
        return copied;
    }
}
