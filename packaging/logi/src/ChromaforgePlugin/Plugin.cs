// The Chromaforge plugin for Logi Options+.
// Copyright (c) 2026 RAD Films - Roman Alurkoff. MIT License.
// Portions from TactileBridge (the author's Capture One plugin): plugin lifecycle and colour
// handling, the focused-key border, the coalesced SDK refresh timer.

using Loupedeck;
using Chromaforge.Surface.Client;

namespace Chromaforge.Logi;

/// <summary>
/// Entry point (found by the Logi Plugin Service by convention). Owns the connection to Chromaforge;
/// the key command and the dial adjustment only forward input to it and draw what it pushes.
/// </summary>
public class ChromaforgePlugin : Plugin
{
    public override bool UsesApplicationApiOnly => true;
    public override bool HasNoApplication => false;

    internal SurfaceClient Client { get; private set; }
    internal PluginConfig Config { get; }

    /// <summary>Raised (coalesced, every 50 ms at most) when the keys and dials need redrawing.</summary>
    internal event Action? Refresh;

    private volatile bool _dirty = true;
    private long _hintAt;
    private volatile string _hint = "Start\nChromaforge";

    /// <summary>The title of an icon-only key (see <c>Renderer.Title</c>): a space unless
    /// config.json's <c>blankTitle</c> says otherwise (re-read every few seconds).</summary>
    internal string BlankTitle => _blank;
    private volatile string _blank = " ";

    /// <summary>The one line a key shows while Chromaforge can't be used (one key only, the rest show
    /// their icons dimmed).</summary>
    internal string Hint() => Client.State switch
    {
        SurfaceState.Pairing => $"Pair:\n{Client.PairCode}",
        _ => _hint,
    };

    private string ComputeHint()
    {
        // short lines, so the key can show them large
        var off = "Chromaforge:\nturn on\ncontrol\nsurfaces";
        if (Client.State == SurfaceState.Refused)
            return Client.Status.Contains("switched off", StringComparison.OrdinalIgnoreCase) ? off : "Chromaforge:\nsee\nSettings";
        if (Client.State == SurfaceState.Disconnected)
        {
            // Chromaforge runs but doesn't listen: its control surfaces are off
            bool running;
            running = ChromaforgeRunning();
            return running ? off : "Start\nChromaforge";
        }
        return "";
    }
    private Timer? _refresh;

    /// <summary>A packaged build runs as <c>chromaforge.exe</c>; a build from source as <c>app.exe</c>
    /// inside a Chromaforge folder. Never throws.</summary>
    private static bool ChromaforgeRunning()
    {
        try
        {
            if (System.Diagnostics.Process.GetProcessesByName("chromaforge").Length > 0) return true;
            foreach (var p in System.Diagnostics.Process.GetProcessesByName("app"))
            {
                try
                {
                    var path = p.MainModule?.FileName ?? "";
                    if (path.Contains("chromaforge", StringComparison.OrdinalIgnoreCase) || path.Contains("\\cf-", StringComparison.OrdinalIgnoreCase)) return true;
                }
                catch (Exception) { /* another user's process: no access to its path */ }
            }
        }
        catch (Exception) { }
        return false;
    }

    public ChromaforgePlugin()
    {
        MigrateFromLightCraft();
        Config = PluginConfig.Load(DataDir());
        Client = NewClient();
        RepairProfiles();
    }

    /// <summary>Zero setup: the Chromaforge profiles the Logi service made before this plugin had
    /// a default for the device get its pass-through actions (see <see cref="ProfileRepair"/>).
    /// Runs while the plugin loads, before the service reads the application profiles.</summary>
    private void RepairProfiles()
    {
        try
        {
            var pluginDir = Path.GetDirectoryName(typeof(ChromaforgePlugin).Assembly.Location) ?? "";
            // ...\LogiPluginService\Plugins\Chromaforge → ...\LogiPluginService\Applications
            var service = Path.GetDirectoryName(Path.GetDirectoryName(pluginDir));
            if (string.IsNullOrEmpty(service)) return;
            var apps = Path.Combine(service, "Applications");
            if (!Directory.Exists(apps)) return;
            ProfileRepair.Run(apps, Path.Combine(pluginDir, "profiles"), Path.Combine(DataDir(), "profile-backups"), s =>
            {
                try { Log?.Info($"[Chromaforge] {s}"); } catch (Exception) { }
            });
        }
        catch (Exception e)
        {
            try { Log?.Warning($"[Chromaforge] profile repair: {e.Message}"); } catch (Exception) { }
        }
    }

    /// <summary>The pairing and port of the plugin's earlier name (see <see cref="LegacyData"/>).</summary>
    private void MigrateFromLightCraft() => LegacyData.Migrate(DataDir(), s =>
    {
        try { Log?.Info($"[Chromaforge] {s}"); } catch (Exception) { }
    });

    private string DataDir()
    {
        try { return GetPluginDataDirectory(); }
        catch (Exception) { return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "ChromaforgeLogi"); }
    }

    private SurfaceClient NewClient()
    {
        var tokens = new FileTokenStore(Path.Combine(DataDir(), "token.txt"));
        // the port is read again before every connection attempt: editing config.json (or Chromaforge's
        // Install / Reinstall writing it) takes effect without restarting anything
        var dir = DataDir();
        return new SurfaceClient(Config.Host, () => PluginConfig.Load(dir).Port, "chromaforge-logi", "Chromaforge plugin for Logi Options+", "mx-creative-console", tokens, s => Log?.Info($"[Chromaforge] {s}"), Path.Combine(DataDir(), "cache"));
    }

    public override void Load()
    {
        PluginEvents.AddEvent(Haptics.KeyPressed, "Key pressed", "A Chromaforge key was pressed");
        PluginEvents.AddEvent(Haptics.ValueReset, "Value reset", "A dial value went back to its default");
        PluginEvents.AddEvent(Haptics.AtLimit, "Value at its limit", "A dial value reached the end of its range");
        Client.LayoutChanged += () => _dirty = true;
        _refresh = new Timer(_ =>
        {
            try { Tick(); }
            catch (Exception e) { Log?.Warning($"[Chromaforge] refresh: {e.Message}"); }
        }, null, TimeSpan.FromMilliseconds(100), TimeSpan.FromMilliseconds(50));
        Client.Start();
    }

    // never throws (see Tick's caller): a timer thread's exception would end the Logi service
    private void Tick()
    {
        {
            if (Environment.TickCount64 - _hintAt > 3000)
            {
                _hintAt = Environment.TickCount64;
                var h = ComputeHint();
                if (h != _hint) { _hint = h; _dirty = true; }
                // config.json points somewhere else now: move there without waiting for a drop
                if (Client.ConnectedPort != 0 && Client.ConnectedPort != Client.WantedPort) Client.Reconnect();
                var cfg = PluginConfig.Load(DataDir());
                if (cfg.BlankTitle != _blank) { _blank = cfg.BlankTitle; _dirty = true; }
                if (cfg.TextContrast != Renderer.Contrast) { Renderer.Contrast = cfg.TextContrast; _dirty = true; }
            }
            if (!_dirty) return;
            _dirty = false;
            try { Refresh?.Invoke(); } catch (Exception e) { Log?.Warning($"[Chromaforge] refresh: {e.Message}"); }
        }
    }

    public override void Unload()
    {
        _refresh?.Dispose();
        Client.Dispose();
    }

    /// <summary>A haptic event, when Chromaforge says haptics are on.</summary>
    internal void Haptic(string name)
    {
        try
        {
            if (Client.Layout?["haptics"]?.GetValue<bool>() == true)
                PluginEvents.RaiseEvent(name);
        }
        catch (Exception) { /* a device without haptics: nothing to do */ }
    }
}

/// <summary>Haptic event names (Logi Options+ maps them to waveforms on devices that have them).</summary>
internal static class Haptics
{
    public const string KeyPressed = "chromaforgeKeyPressed";
    public const string ValueReset = "chromaforgeValueReset";
    public const string AtLimit = "chromaforgeAtLimit";
}

/// <summary>Where Chromaforge listens (the plugin data folder's <c>config.json</c>; defaults to 127.0.0.1:7981).</summary>
internal sealed class PluginConfig
{
    public string Host { get; init; } = "127.0.0.1";
    public int Port { get; init; } = 7981;
    public string BlankTitle { get; init; } = " ";
    /// <summary>none (default), outline, shadow or strip: see <c>Renderer.Contrast</c>.</summary>
    public string TextContrast { get; init; } = "none";

    public static PluginConfig Load(string dir)
    {
        var c = new PluginConfig();
        try
        {
            // LIGHTCRAFT_SURFACE_PORT: the name before the rename, still read
            var env = Environment.GetEnvironmentVariable("CHROMAFORGE_SURFACE_PORT") ?? Environment.GetEnvironmentVariable("LIGHTCRAFT_SURFACE_PORT");
            var path = Path.Combine(dir, "config.json");
            int port = c.Port;
            var blank = c.BlankTitle;
            var contrast = c.TextContrast;
            if (File.Exists(path))
            {
                var j = System.Text.Json.Nodes.JsonNode.Parse(File.ReadAllText(path));
                if (j?["port"]?.GetValue<int>() is int p && p is > 0 and < 65536) port = p;
                if (j?["blankTitle"]?.ToString() is { Length: > 0 and < 8 } b) blank = b;
                if (j?["textContrast"]?.ToString() is "outline" or "shadow" or "strip" or "none") contrast = j["textContrast"]!.ToString();
            }
            if (int.TryParse(env, out var e) && e is > 0 and < 65536) port = e;
            return new PluginConfig { Port = port, BlankTitle = blank, TextContrast = contrast };
        }
        catch (Exception) { return c; }
    }
}

public class ChromaforgeApplication : ClientApplication
{
    public ChromaforgeApplication() { }

    protected override string GetProcessName() => "chromaforge";

    public override string GetDisplayName() => "Chromaforge";
}
