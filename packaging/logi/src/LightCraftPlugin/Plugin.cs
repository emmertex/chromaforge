// The LightCraft plugin for Logi Options+.
// Copyright (c) 2026 RAD Films - Roman Alurkoff. MIT License.
// Portions from TactileBridge (the author's Capture One plugin): plugin lifecycle and colour
// handling, the focused-key border, the coalesced SDK refresh timer.

using Loupedeck;
using LightCraft.Surface.Client;

namespace LightCraft.Logi;

/// <summary>
/// Entry point (found by the Logi Plugin Service by convention). Owns the connection to LightCraft;
/// the key command and the dial adjustment only forward input to it and draw what it pushes.
/// </summary>
public class LightCraftPlugin : Plugin
{
    public override bool UsesApplicationApiOnly => true;
    public override bool HasNoApplication => false;

    internal SurfaceClient Client { get; private set; }
    internal PluginConfig Config { get; }

    /// <summary>Raised (coalesced, every 50 ms at most) when the keys and dials need redrawing.</summary>
    internal event Action? Refresh;

    private volatile bool _dirty = true;
    private long _hintAt;
    private volatile string _hint = "Start\nLightCraft";

    /// <summary>The title of an icon-only key (see <c>Renderer.Title</c>): a space unless
    /// config.json's <c>blankTitle</c> says otherwise (re-read every few seconds).</summary>
    internal string BlankTitle => _blank;
    private volatile string _blank = " ";

    /// <summary>The one line a key shows while LightCraft can't be used (one key only, the rest show
    /// their icons dimmed).</summary>
    internal string Hint() => Client.State switch
    {
        SurfaceState.Pairing => $"Pair:\n{Client.PairCode}",
        _ => _hint,
    };

    private string ComputeHint()
    {
        // short lines, so the key can show them large
        var off = "LightCraft:\nturn on\ncontrol\nsurfaces";
        if (Client.State == SurfaceState.Refused)
            return Client.Status.Contains("switched off", StringComparison.OrdinalIgnoreCase) ? off : "LightCraft:\nsee\nSettings";
        if (Client.State == SurfaceState.Disconnected)
        {
            // LightCraft runs but doesn't listen: its control surfaces are off
            bool running;
            try { running = System.Diagnostics.Process.GetProcessesByName("lightcraft").Length > 0; }
            catch (Exception) { running = false; }
            return running ? off : "Start\nLightCraft";
        }
        return "";
    }
    private Timer? _refresh;

    public LightCraftPlugin()
    {
        Config = PluginConfig.Load(DataDir());
        Client = NewClient();
        RepairProfiles();
    }

    /// <summary>Zero setup: the LightCraft profiles the Logi service made before this plugin had
    /// a default for the device get its pass-through actions (see <see cref="ProfileRepair"/>).
    /// Runs while the plugin loads, before the service reads the application profiles.</summary>
    private void RepairProfiles()
    {
        try
        {
            var pluginDir = Path.GetDirectoryName(typeof(LightCraftPlugin).Assembly.Location) ?? "";
            // ...\LogiPluginService\Plugins\LightCraft → ...\LogiPluginService\Applications
            var service = Path.GetDirectoryName(Path.GetDirectoryName(pluginDir));
            if (string.IsNullOrEmpty(service)) return;
            var apps = Path.Combine(service, "Applications");
            if (!Directory.Exists(apps)) return;
            ProfileRepair.Run(apps, Path.Combine(pluginDir, "profiles"), Path.Combine(DataDir(), "profile-backups"), s =>
            {
                try { Log?.Info($"[LightCraft] {s}"); } catch (Exception) { }
            });
        }
        catch (Exception e)
        {
            try { Log?.Warning($"[LightCraft] profile repair: {e.Message}"); } catch (Exception) { }
        }
    }

    private string DataDir()
    {
        try { return GetPluginDataDirectory(); }
        catch (Exception) { return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "LightCraftLogi"); }
    }

    private SurfaceClient NewClient()
    {
        var tokens = new FileTokenStore(Path.Combine(DataDir(), "token.txt"));
        // the port is read again before every connection attempt: editing config.json (or LightCraft's
        // Install / Reinstall writing it) takes effect without restarting anything
        var dir = DataDir();
        return new SurfaceClient(Config.Host, () => PluginConfig.Load(dir).Port, "lightcraft-logi", "LightCraft plugin for Logi Options+", "mx-creative-console", tokens, s => Log?.Info($"[LightCraft] {s}"), Path.Combine(DataDir(), "cache"));
    }

    public override void Load()
    {
        PluginEvents.AddEvent(Haptics.KeyPressed, "Key pressed", "A LightCraft key was pressed");
        PluginEvents.AddEvent(Haptics.ValueReset, "Value reset", "A dial value went back to its default");
        PluginEvents.AddEvent(Haptics.AtLimit, "Value at its limit", "A dial value reached the end of its range");
        Client.LayoutChanged += () => _dirty = true;
        _refresh = new Timer(_ =>
        {
            try { Tick(); }
            catch (Exception e) { Log?.Warning($"[LightCraft] refresh: {e.Message}"); }
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
            try { Refresh?.Invoke(); } catch (Exception e) { Log?.Warning($"[LightCraft] refresh: {e.Message}"); }
        }
    }

    public override void Unload()
    {
        _refresh?.Dispose();
        Client.Dispose();
    }

    /// <summary>A haptic event, when LightCraft says haptics are on.</summary>
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
    public const string KeyPressed = "lightcraftKeyPressed";
    public const string ValueReset = "lightcraftValueReset";
    public const string AtLimit = "lightcraftAtLimit";
}

/// <summary>Where LightCraft listens (the plugin data folder's <c>config.json</c>; defaults to 127.0.0.1:7981).</summary>
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
            var env = Environment.GetEnvironmentVariable("LIGHTCRAFT_SURFACE_PORT");
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

public class LightCraftApplication : ClientApplication
{
    public LightCraftApplication() { }

    protected override string GetProcessName() => "lightcraft";

    public override string GetDisplayName() => "LightCraft";
}
