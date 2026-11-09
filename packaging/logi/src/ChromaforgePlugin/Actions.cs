// Chromaforge's actions in the Logi Options+ actions panel, by category.
// Copyright (c) 2026 RAD Films - Roman Alurkoff. MIT License.
// Portions from TactileBridge: numbered action categories so Options+ keeps their order.

using System.Text.Json.Nodes;
using Loupedeck;

namespace Chromaforge.Logi;

/// <summary>One action of Chromaforge's catalogue (<c>surface.catalog</c>).</summary>
internal sealed record CatalogEntry(string Key, string Group, string Label, string? Icon, bool Value);

/// <summary>The catalogue as the actions panel lists it: numbered categories.</summary>
internal static class CatalogView
{
    public static List<CatalogEntry> Entries(JsonNode? catalog, bool values)
    {
        var list = new List<CatalogEntry>();
        if (catalog?["entries"] is not JsonArray all) return list;
        var order = new List<string>();
        if (catalog["groups"] is JsonArray g)
            foreach (var x in g)
                if (x?.ToString() is { } name) order.Add(name);
        foreach (var e in all)
        {
            var key = e?["key"]?.ToString();
            if (string.IsNullOrEmpty(key) || key.Contains("___")) continue;
            var isValue = e?["value"]?.GetValueKind() == System.Text.Json.JsonValueKind.True;
            if (isValue != values) continue;
            var group = e?["group"]?.ToString() ?? "";
            var n = order.IndexOf(group);
            // "1. White Balance": Options+ sorts categories by name
            var numbered = n >= 0 ? $"{n + 1}. {group}" : group;
            list.Add(new CatalogEntry(key, numbered, e?["label"]?.ToString() ?? key, e?["icon"]?.ToString(), isValue));
        }
        return list;
    }

    /// <summary>The value an action key moves (<c>v.light.exposure</c> → <c>light.exposure</c>).</summary>
    public static string? Target(string key) => key.StartsWith("v.", StringComparison.Ordinal) ? key[2..] : null;
}

/// <summary>
/// Chromaforge's key actions (ratings, labels, browsing, tools, masks, view…), one Options+ action
/// each, for putting them on keys in Options+ directly. The list comes from Chromaforge and is
/// kept from the last run, so it is there before Chromaforge starts.
/// </summary>
public class ChromaforgeAction : PluginDynamicCommand
{
    private readonly Dictionary<string, CatalogEntry> _entries = new(StringComparer.Ordinal);

    public ChromaforgeAction() : base()
    {
        Name = "ChromaforgeAction";
        DisplayName = "Chromaforge Action";
        Description = "A Chromaforge action";
    }

    private ChromaforgePlugin? Owner => Plugin as ChromaforgePlugin;

    protected override bool OnLoad()
    {
        if (Owner is { } p)
        {
            Fill(p);
            p.Client.CatalogChanged += () => Fill(p);
            // names and icons only change with the catalogue (Fill); nothing to redraw per frame
        }
        return true;
    }

    private void Fill(ChromaforgePlugin p)
    {
        lock (_entries)
        {
            RemoveAllParameters();
            _entries.Clear();
            foreach (var e in CatalogView.Entries(p.Client.Catalog, values: false))
            {
                if (!_entries.TryAdd(e.Key, e)) continue;
                AddParameter(e.Key, e.Label, e.Group);
            }
        }
        ParametersChanged();
    }

    private CatalogEntry? Entry(string key)
    {
        lock (_entries) return _entries.TryGetValue(key, out var e) ? e : null;
    }

    protected override bool ProcessButtonEvent2(string actionParameter, DeviceButtonEvent2 buttonEvent)
    {
        if (Owner is not { } p || Entry(actionParameter) == null) return false;
        if (buttonEvent.EventType == DeviceButtonEventType.Press)
        {
            p.Client.PressAction(actionParameter, true);
            p.Haptic(Haptics.KeyPressed);
        }
        else if (buttonEvent.EventType == DeviceButtonEventType.Release)
        {
            p.Client.PressAction(actionParameter, false);
        }
        return true;
    }

    protected override void RunCommand(string actionParameter)
    {
        if (Owner is not { } p || Entry(actionParameter) == null) return;
        p.Client.PressAction(actionParameter, true);
        p.Client.PressAction(actionParameter, false);
        p.Haptic(Haptics.KeyPressed);
    }

    // the name under the icon, as TactileBridge's action keys; an action without an icon shows its name as the image
    protected override string GetCommandDisplayName(string actionParameter, PluginImageSize imageSize) =>
        Owner is { } p && Entry(actionParameter) is { } e ? (Renderer.Icon(p.Client, e.Icon) == null ? p.BlankTitle : e.Label) : " ";

    protected override BitmapImage GetCommandImage(string actionParameter, PluginImageSize imageSize)
    {
        if (Owner is not { } p) return new BitmapBuilder(imageSize).ToImage();
        var e = Entry(actionParameter);
        return Renderer.Action(p, e?.Icon, e?.Label ?? actionParameter, imageSize);
    }
}

/// <summary>
/// Chromaforge's values (every develop slider, mask, brush and view value), one Options+ dial
/// action each: turning moves the value on the open photo (one undo step per turn), pressing
/// resets it, and the display shows the live value.
/// </summary>
public class ChromaforgeAdjust : PluginDynamicAdjustment
{
    private readonly Dictionary<string, CatalogEntry> _entries = new(StringComparer.Ordinal);
    private readonly HashSet<string> _shown = new(StringComparer.Ordinal);
    private readonly Redraw _values = new();
    private readonly Redraw _images = new();

    public ChromaforgeAdjust() : base(hasReset: true)
    {
        Name = "ChromaforgeAdjust";
        DisplayName = "Chromaforge Value";
        Description = "A Chromaforge value";
    }

    private ChromaforgePlugin? Owner => Plugin as ChromaforgePlugin;

    protected override bool OnLoad()
    {
        if (Owner is { } p)
        {
            Fill(p);
            p.Client.CatalogChanged += () => Fill(p);
            // the values on show change: redraw those (each by name; see ChromaforgeKey)
            p.Refresh += () =>
            {
                string[] shown;
                lock (_shown) shown = _shown.Select(t => "v." + t).ToArray();
                // only what changed: the value text when the value moved, the image (the icon
                // alone) when the icons or colours did
                foreach (var n in shown)
                {
                    if (_values.Changed(n, Live(p, n)?.ToJsonString() ?? "")) AdjustmentValueChanged(n);
                    if (_images.Changed(n, Redraw.Signature(p, null, false, null))) ActionImageChanged(n);
                }
            };
        }
        return true;
    }

    private void Fill(ChromaforgePlugin p)
    {
        lock (_entries)
        {
            RemoveAllParameters();
            _entries.Clear();
            foreach (var e in CatalogView.Entries(p.Client.Catalog, values: true))
            {
                if (!_entries.TryAdd(e.Key, e)) continue;
                AddParameter(e.Key, e.Label, e.Group);
            }
        }
        ParametersChanged();
    }

    private CatalogEntry? Entry(string key)
    {
        lock (_entries) return _entries.TryGetValue(key, out var e) ? e : null;
    }

    /// <summary>Ask Chromaforge for the live value of every value the device shows.</summary>
    private void Shown(ChromaforgePlugin p, string key)
    {
        if (CatalogView.Target(key) is not { } t) return;
        string[] all;
        lock (_shown)
        {
            if (!_shown.Add(t)) return;
            all = _shown.ToArray();
        }
        p.Client.Watch(all);
    }

    private static JsonNode? Live(ChromaforgePlugin p, string key) =>
        CatalogView.Target(key) is { } t ? p.Client.Layout?["values"]?[t] : null;

    protected override void ApplyAdjustment(string actionParameter, int diff)
    {
        if (Owner is not { } p || diff == 0 || Entry(actionParameter) == null) return;
        Shown(p, actionParameter);
        p.Client.TurnAction(actionParameter, diff);
        var pos = Live(p, actionParameter)?["position"]?.GetValue<double>();
        if (pos is <= 0.0 or >= 1.0) p.Haptic(Haptics.AtLimit);
    }

    protected override void RunCommand(string actionParameter)
    {
        // the dial pressed: back to its default
        if (Owner is not { } p || Entry(actionParameter) == null) return;
        p.Client.PressAction(actionParameter, true);
        p.Client.PressAction(actionParameter, false);
        p.Haptic(Haptics.ValueReset);
    }

    protected override string GetAdjustmentValue(string actionParameter)
    {
        if (Owner is not { } p) return "";
        Shown(p, actionParameter);
        return Live(p, actionParameter)?["value"]?.ToString() ?? "";
    }

    // TactileBridge's value keys are their icon alone (the list names them by their parameter)
    protected override string GetAdjustmentDisplayName(string actionParameter, PluginImageSize imageSize) =>
        Owner is { } p ? p.BlankTitle : actionParameter;

    protected override BitmapImage GetAdjustmentImage(string actionParameter, PluginImageSize imageSize)
    {
        if (Owner is not { } p) return new BitmapBuilder(imageSize).ToImage();
        Shown(p, actionParameter);
        var e = Entry(actionParameter);
        return Renderer.Action(p, e?.Icon, e?.Label ?? actionParameter, imageSize);
    }
}
