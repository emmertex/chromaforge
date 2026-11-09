// Zero setup for the Loupedeck+: its LightCraft profile gets the plugin's pass-through actions.
// Copyright (c) 2026 RAD Films - Roman Alurkoff. MIT License.

using System.IO.Compression;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace LightCraft.Logi;

/// <summary>
/// The Logi Plugin Service sends a control's turns and presses to the plugin only when one of the
/// plugin's actions is assigned to that control in the device's profile. The plugin ships default
/// profiles (<c>profiles/DefaultProfileNN.lp5</c>) that assign the pass-through actions to every
/// control, but the service uses a default profile only when it creates the application's
/// profile, the first time it sees the plugin. A LightCraft profile created by an older plugin
/// (one without a Loupedeck+ default) stays empty forever: nothing on the Loupedeck+ reached
/// LightCraft until the user assigned each control by hand.
///
/// So, when the plugin loads (before the service reads the application profiles), every
/// LightCraft profile of a device the plugin has a default for gets the default's pass-through
/// actions on the controls that have none, or that hold one of LightCraft's own per-value actions
/// (assigned by hand only to get a knob working; LightCraft maps the knob itself). Controls with
/// any other action (another plugin, a keyboard shortcut) are left as they are. The original file
/// is copied to the plugin's data folder first (<c>profile-backups</c>). Never throws.
/// </summary>
internal static class ProfileRepair
{
    /// <summary>Devices whose profile is repaired: the Loupedeck+ and the Actions Ring (their
    /// defaults came after the first release; the Keypad's and the Dialpad's were in it).</summary>
    public static readonly string[] Devices = { "Loupedeck15", "Loupedeck72" };

    /// <summary>LightCraft actions a control may hold only as a stand-in for the pass-through.</summary>
    private static readonly string[] StandIns = { "$LightCraft___LightCraftAdjust___", "$LightCraft___ResetLightCraftAdjust___" };

    private const int MaxProfileBytes = 32 << 20;

    /// <summary>Repair what needs it; returns one line per profile changed.</summary>
    /// <param name="applications"><c>%LOCALAPPDATA%\Logi\LogiPluginService\Applications</c></param>
    /// <param name="profilesDir">the plugin's own <c>profiles</c> folder (the .lp5 defaults)</param>
    /// <param name="backupDir">where originals are copied before a change</param>
    public static List<string> Run(string applications, string profilesDir, string backupDir, Action<string>? log)
    {
        var done = new List<string>();
        foreach (var device in Devices)
        {
            try
            {
                var number = device["Loupedeck".Length..];
                var defaults = DefaultSlots(Path.Combine(profilesDir, $"DefaultProfile{number}.lp5"));
                if (defaults == null) continue;
                var dir = Path.Combine(applications, device, "@_lightcraft", "Profiles");
                if (!Directory.Exists(dir)) continue;
                foreach (var profile in Directory.EnumerateDirectories(dir))
                {
                    var file = Path.Combine(profile, "ProfileInfo.json");
                    if (!File.Exists(file) || new FileInfo(file).Length > MaxProfileBytes) continue;
                    var text = File.ReadAllText(file);
                    var (fixedText, changed) = Repair(text, defaults);
                    if (changed == 0 || fixedText == null) continue;
                    Directory.CreateDirectory(backupDir);
                    var backup = Path.Combine(backupDir, $"{device}-{Path.GetFileName(profile)}-{DateTime.Now:yyyyMMdd-HHmmss}.json");
                    File.WriteAllText(backup, text);
                    var tmp = file + ".lightcraft.tmp";
                    File.WriteAllText(tmp, fixedText);
                    File.Move(tmp, file, true);
                    var line = $"{device} profile {Path.GetFileName(profile)}: {changed} controls now reach LightCraft (original kept in {backup})";
                    done.Add(line);
                    log?.Invoke(line);
                }
            }
            catch (Exception e)
            {
                log?.Invoke($"profile repair ({device}): {e.Message}");
            }
        }
        return done;
    }

    /// <summary>The default profile's actions of its first workspace, per slot list: the
    /// Loupedeck+'s flat lists (<c>pressActions</c>, <c>rotateActions</c>, by control number) and
    /// the paged devices' pages (<c>pressPages/0/pressAction</c>, by control id); null if it can't
    /// be read.</summary>
    public static Dictionary<string, string?[]>? DefaultSlots(string lp5)
    {
        if (!File.Exists(lp5)) return null;
        using var zip = ZipFile.OpenRead(lp5);
        var entry = zip.GetEntry("ProfileInfo.json");
        if (entry == null || entry.Length > MaxProfileBytes) return null;
        using var r = new StreamReader(entry.Open());
        var root = JsonNode.Parse(r.ReadToEnd());
        var ws = Workspaces(root).FirstOrDefault();
        if (ws == null) return null;
        var slots = new Dictionary<string, string?[]>(StringComparer.Ordinal);
        foreach (var (name, value) in ws)
        {
            if (name.EndsWith("Actions", StringComparison.Ordinal) && value is JsonArray a)
                slots[name] = a.Select(x => x?.GetValueKind() == JsonValueKind.String ? x.ToString() : null).ToArray();
            if (name.EndsWith("Pages", StringComparison.Ordinal) && value is JsonArray pages)
            {
                for (int p = 0; p < pages.Count; p++)
                {
                    if (pages[p]?["controls"] is not JsonArray controls) continue;
                    foreach (var field in new[] { "pressAction", "rotateAction" })
                    {
                        var byId = new string?[128];
                        foreach (var c in controls)
                            if (c?["controlId"]?.GetValue<int>() is int id and >= 0 and < 128 && c[field]?.GetValueKind() == JsonValueKind.String)
                                byId[id] = c[field]!.ToString();
                        if (byId.Any(x => x != null)) slots[$"{name}/{p}/{field}"] = byId;
                    }
                }
            }
        }
        return slots;
    }

    /// <summary>A profile's text with the pass-through actions filled in, and how many controls
    /// changed (0: leave the file alone).</summary>
    public static (string? text, int changed) Repair(string profileJson, Dictionary<string, string?[]> defaults)
    {
        var root = JsonNode.Parse(profileJson);
        int changed = 0;
        foreach (var ws in Workspaces(root))
        {
            foreach (var (name, want) in defaults)
            {
                if (name.Split('/') is [var pagesName, var page, var field] && int.TryParse(page, out var p))
                {
                    changed += RepairPage((ws[pagesName] as JsonArray)?.ElementAtOrDefault(p)?["controls"] as JsonArray, field, want);
                    continue;
                }
                if (ws[name] is not JsonArray have) continue;
                for (int i = 0; i < Math.Min(have.Count, want.Length); i++)
                {
                    if (want[i] is not { } action) continue;
                    var cur = have[i]?.GetValueKind() == JsonValueKind.String ? have[i]!.ToString() : null;
                    if (cur == action) continue;
                    bool free = have[i] == null || (cur != null && StandIns.Any(s => cur.StartsWith(s, StringComparison.Ordinal)));
                    if (!free) continue;
                    have[i] = action;
                    changed++;
                }
            }
        }
        if (changed == 0) return (null, 0);
        return (root!.ToJsonString(new JsonSerializerOptions { WriteIndented = true, Encoder = System.Text.Encodings.Web.JavaScriptEncoder.UnsafeRelaxedJsonEscaping }), changed);
    }

    /// <summary>A paged device's page: each control (by its id) whose action is empty, or a
    /// stand-in, gets the default's.</summary>
    private static int RepairPage(JsonArray? controls, string field, string?[] want)
    {
        if (controls == null) return 0;
        int changed = 0;
        foreach (var c in controls)
        {
            if (c is not JsonObject o || o["controlId"]?.GetValueKind() != JsonValueKind.Number) continue;
            var id = o["controlId"]!.GetValue<int>();
            if (id < 0 || id >= want.Length || want[id] is not { } action) continue;
            var cur = o[field]?.GetValueKind() == JsonValueKind.String ? o[field]!.ToString() : null;
            if (cur == action) continue;
            bool free = o[field] == null || (cur != null && StandIns.Any(s => cur.StartsWith(s, StringComparison.Ordinal)));
            if (!free) continue;
            o[field] = action;
            changed++;
        }
        return changed;
    }

    /// <summary>Every workspace object of every layout mode (<c>layout.layoutModes[].workspaces[]</c>).</summary>
    private static IEnumerable<JsonObject> Workspaces(JsonNode? root)
    {
        if (root?["layout"]?["layoutModes"] is not JsonArray modes) yield break;
        foreach (var m in modes)
            if (m?["workspaces"] is JsonArray wss)
                foreach (var w in wss)
                    if (w is JsonObject o) yield return o;
    }
}
