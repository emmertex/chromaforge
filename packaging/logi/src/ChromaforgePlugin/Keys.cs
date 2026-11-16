// Chromaforge keys, buttons and dials for Logi Options+.
// Copyright (c) 2026 RAD Films - Roman Alurkoff. MIT License.
// Portions from TactileBridge: the focused-key rounded border and the key image layout.

using System.Text.Json.Nodes;
using Loupedeck;
using Chromaforge.Surface.Client;

namespace Chromaforge.Logi;

/// <summary>Action groups in the Options+ actions panel (numbered: Options+ sorts them by name).</summary>
internal static class Groups
{
    public const string Keys = "0. Keys and dials set in Chromaforge";
}

/// <summary>
/// Draws keys the way TactileBridge's Capture One plugin does: the icon itself is the key's image
/// (a vector the Logi service scales to the key, so it fills the key like TactileBridge's icons),
/// and the action's name or live value is the key's title, which Logi Options+ and the device
/// draw under it. A key whose value the dial moves gets TactileBridge's rounded border.
/// </summary>
internal static class Renderer
{
    private static readonly Dictionary<string, BitmapImage?> Icons = new();
    private static readonly object IconLock = new();

    public static BitmapColor Color(string? hex, BitmapColor fallback)
    {
        if (string.IsNullOrEmpty(hex)) return fallback;
        var h = hex.TrimStart('#');
        return h.Length == 6 && uint.TryParse(h, System.Globalization.NumberStyles.HexNumber, null, out var rgb)
            ? new BitmapColor((int)(rgb >> 16) & 0xFF, (int)(rgb >> 8) & 0xFF, (int)rgb & 0xFF)
            : fallback;
    }

    /// <summary>The SVG wrapped in a faded group (Chromaforge not running).</summary>
    private static string Faded(string svg)
    {
        var open = svg.IndexOf("<svg", StringComparison.OrdinalIgnoreCase);
        var tagEnd = open >= 0 ? svg.IndexOf('>', open) : -1;
        var close = svg.LastIndexOf("</svg>", StringComparison.OrdinalIgnoreCase);
        if (tagEnd < 0 || close < tagEnd) return svg;
        return svg[..(tagEnd + 1)] + "<g opacity=\"0.35\">" + svg[(tagEnd + 1)..close] + "</g>" + svg[close..];
    }

    /// <summary>An icon of Chromaforge's pack as a vector image (cached; <paramref name="dim"/>: faded).</summary>
    public static BitmapImage? Icon(SurfaceClient client, string? name, bool dim = false)
    {
        if (string.IsNullOrEmpty(name)) return null;
        var key = dim ? name + "\u0000dim" : name;
        lock (IconLock)
        {
            if (Icons.TryGetValue(key, out var cached)) return cached;
            BitmapImage? img = null;
            var svg = client.Icon(name);
            if (svg == null) return null;
            try { img = BitmapImage.FromSvg(dim ? Faded(svg) : svg); } catch (Exception) { img = null; }
            Icons[key] = img;
            return img;
        }
    }

    private static BitmapImage Blank(PluginImageSize size, BitmapColor bg)
    {
        using var b = new BitmapBuilder(size);
        b.Clear(bg);
        return b.ToImage();
    }

    /// <summary>Text filling a key (a hint, or an action without an icon).</summary>
    public static BitmapImage Text(PluginImageSize size, string text, BitmapColor bg, BitmapColor fg)
    {
        using var b = new BitmapBuilder(size);
        b.Clear(bg);
        int w = b.Width, h = b.Height;
        // a name instead of an icon: as large as fits, on up to three balanced lines, bold
        var lines = text.Contains('\n') ? text.Split('\n').ToList() : Wrap(text);
        int longest = Math.Max(1, lines.Max(l => l.Length));
        int byHeight = (int)(h * 0.86 / (lines.Count * 1.15));
        int byWidth = (int)((w - 6) / (longest * 0.56));
        int font = Math.Clamp(Math.Min(byHeight, byWidth), 9, h / 2);
        var joined = string.Join("\n", lines);
        // bold: the text twice, a pixel apart
        b.DrawText(joined, 3, 0, w - 6, h, fg, font, (int)(font * 1.15), 0, null);
        b.DrawText(joined, 4, 0, w - 6, h, fg, font, (int)(font * 1.15), 0, null);
        return b.ToImage();
    }

    /// <summary>Words on 1–3 lines, the longest line as short as possible.</summary>
    internal static List<string> Wrap(string text)
    {
        var words = text.Split(' ', StringSplitOptions.RemoveEmptyEntries);
        if (words.Length <= 1 || text.Length <= 8) return new List<string> { text };
        List<string>? best = null;
        int bestLen = int.MaxValue;
        for (int n = 1; n <= Math.Min(3, words.Length); n++)
        {
            // greedy split into n lines aiming at text.Length / n characters each
            int target = (int)Math.Ceiling(text.Length / (double)n);
            var lines = new List<string>();
            var cur = "";
            foreach (var word in words)
            {
                var next = cur.Length == 0 ? word : cur + " " + word;
                if (cur.Length > 0 && next.Length > target && lines.Count < n - 1)
                {
                    lines.Add(cur);
                    cur = word;
                }
                else
                {
                    cur = next;
                }
            }
            lines.Add(cur);
            // fewer lines wins a tie; more lines only when they shorten the longest clearly
            int len = lines.Max(l => l.Length) + lines.Count * 2;
            if (len < bestLen)
            {
                bestLen = len;
                best = lines;
            }
        }
        return best ?? new List<string> { text };
    }

    /// <summary>A key's image (the title carries its name or value: see <see cref="Title"/>).</summary>
    public static BitmapImage Key(SurfaceClient client, JsonNode? view, PluginImageSize size, bool dim)
    {
        var layout = client.LastLayout;
        var bg = Color(layout?["background"]?.ToString(), BitmapColor.Black);
        var fg = Color(layout?["foreground"]?.ToString(), BitmapColor.White);
        if (view == null || view["binding"] == null) return Blank(size, bg);
        var icon = Icon(client, view["icon"]?.ToString(), dim);
        // no icon: the name is the image (and the title stays empty)
        if (icon == null) return Text(size, view["label"]?.ToString() ?? "", bg, dim ? new BitmapColor(fg.R, fg.G, fg.B, 110) : fg);
        var state = LitState(view, dim);
        using var b = new BitmapBuilder(size);
        int w = b.Width, h = b.Height;
        var (ix, iy, iside) = IconArea(w, h);
        if (state == "focus")
        {
            // TactileBridge's lit key (its value is on the dial): the whole key white and rounded,
            // a black rounded inset above a white foot, the icon where an unlit key has it, and
            // the value in DARK text on the white foot (white on white never reads). One
            // full-size image with no title.
            int edge = Math.Max(3, w / 14);
            int foot = (int)(h * 0.74);
            RoundedRect(b, 0, 0, w, h, Math.Max(6, w / 7), BitmapColor.White);
            RoundedRect(b, edge, edge, w - 2 * edge, foot - edge, Math.Max(4, (int)(w * 0.28)), BitmapColor.Black);
            b.DrawImage(icon, ix, iy, iside, iside, BitmapRotation.None);
            var text = view!["value"]?.ToString() is { Length: > 0 } v ? v : view["label"]?.ToString() ?? "";
            if (text.Length > 0)
            {
                int band = h - foot - edge / 2;
                int font = Math.Max(9, Math.Min((int)(band * 0.8), (int)((w - 2 * edge) / (text.Length * 0.6))));
                b.DrawText(text, edge, foot, w - 2 * edge, band, BitmapColor.Black, font, (int)(font * 1.15), 0, null);
                b.DrawText(text, edge + 1, foot, w - 2 * edge, band, BitmapColor.Black, font, (int)(font * 1.15), 0, null);
            }
            return b.ToImage();
        }
        b.Clear(bg);
        if (state is "on" or "held")
        {
            // a mode that is on, or a key held down: a coloured ring around the normal key, not the
            // white "on the dial" look, so the two never get mixed up
            int ring = Math.Max(3, w / 18);
            int r = Math.Max(6, w / 7);
            var accent = state == "held" ? new BitmapColor(80, 200, 255) : new BitmapColor(255, 120, 1);
            RoundedRect(b, 0, 0, w, h, r, accent);
            RoundedRect(b, ring, ring, w - 2 * ring, h - 2 * ring, Math.Max(2, r - ring), bg);
        }
        b.DrawImage(icon, ix, iy, iside, iside, BitmapRotation.None);
        // the title drawn into the image when a contrast treatment is chosen (config textContrast)
        if (Contrast != "none" && TitleText(view, dim) is { Length: > 0 } title)
            DrawFootText(b, title, fg, Contrast);
        return b.ToImage();
    }

    /// <summary>How text over the foot of an icon is made to read (config.json <c>textContrast</c>):
    /// <c>none</c> (the device draws the title, the default), <c>outline</c>, <c>shadow</c> or
    /// <c>strip</c> (the plugin draws it into the image).</summary>
    public static volatile string Contrast = "none";

    /// <summary>A key's text at its foot, with a contrast treatment.</summary>
    public static void DrawFootText(BitmapBuilder b, string text, BitmapColor fg, string mode)
    {
        int w = b.Width, h = b.Height;
        int top = (int)(h * 0.70), band = h - top - 2;
        int font = Math.Max(9, Math.Min((int)(band * 0.62), (int)((w - 6) / (Math.Max(1, text.Length) * 0.58))));
        var dark = new BitmapColor(0, 0, 0);
        void At(int dx, int dy, BitmapColor c) => b.DrawText(text, 3 + dx, top + dy, w - 6, band, c, font, (int)(font * 1.15), 0, null);
        switch (mode)
        {
            case "outline":
                foreach (var (dx, dy) in new[] { (-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1) })
                    At(dx, dy, dark);
                break;
            case "shadow":
                At(2, 2, new BitmapColor(0, 0, 0, 110));
                At(1, 1, new BitmapColor(0, 0, 0, 200));
                break;
            case "strip":
                b.FillRectangle(0, top + band / 6, w, band * 2 / 3 + 2, new BitmapColor(0, 0, 0, 150));
                break;
        }
        At(0, 0, fg);
    }

    /// <summary>How a key is lit: <c>focus</c>, <c>on</c>, <c>held</c> or none (see Chromaforge's
    /// KeyView.state; older Chromaforge sends only <c>active</c>: focus).</summary>
    public static string? LitState(JsonNode? view, bool dim)
    {
        if (dim || view?["active"]?.GetValue<bool>() != true) return null;
        return view["state"]?.ToString() is { Length: > 0 } st ? st : "focus";
    }

    /// <summary>Where a key's icon goes: TactileBridge's value-key icon area (86 % of the key, 4 %
    /// from the top), leaving the bottom for the title the template overlays (metadata/
    /// DefaultIconTemplate.ict: the image fills the whole key, the title is drawn over its foot).</summary>
    public static (int x, int y, int side) IconArea(int w, int h)
    {
        int side = (int)(Math.Min(w, h) * 0.86);
        return ((w - side) / 2, (int)(h * 0.04), side);
    }

    /// <summary>An unlit key: the icon in its area on the key's own background, as one full-size image.</summary>
    public static BitmapImage Face(BitmapImage icon, PluginImageSize size, BitmapColor bg)
    {
        using var b = new BitmapBuilder(size);
        b.Clear(bg);
        var (x, y, side) = IconArea(b.Width, b.Height);
        b.DrawImage(icon, x, y, side, side, BitmapRotation.None);
        return b.ToImage();
    }

    /// <summary>Drawn in the white "on the dial" style (its text is inside the image).</summary>
    public static bool Lit(JsonNode? view, bool dim) => LitState(view, dim) == "focus";

    /// <summary>A key's title: its live value when it shows one (as TactileBridge's value keys), else
    /// its name; empty when the image already says it (no icon) or the key does nothing.</summary>
    public static string Title(ChromaforgePlugin plugin, JsonNode? view, bool dim)
    {
        // An empty name makes Options+ show the parameter's own name ("Keypad page 1, key 1"):
        // a blank title (a space) keeps the key icon-only, which also lets the icon fill the key.
        var blank = plugin.BlankTitle;
        if (view == null || view["binding"] == null) return blank;
        var icon = plugin.Client.Icon(view["icon"]?.ToString() ?? "") != null;
        if (!icon) return blank;
        // a lit key, or a contrast treatment, carries its text inside its image
        if (Lit(view, dim) || Contrast != "none") return blank;
        return TitleText(view, dim) is { Length: > 0 } t ? t : blank;
    }

    /// <summary>TactileBridge: a value key shows its live value under the icon, all the time, and
    /// no name; an action key shows its name under the icon ("Undo", "Toggle Auto Sync").</summary>
    public static string? TitleText(JsonNode? view, bool dim)
    {
        if (view?["binding"] == null) return null;
        if (view["binding"]?["kind"]?.ToString() == "value")
            return !dim && view["value"]?.ToString() is { Length: > 0 } value ? value : null;
        return view["label"]?.ToString();
    }

    /// <summary>A catalogue action's image: its icon, or its name when it has none.</summary>
    public static BitmapImage Action(ChromaforgePlugin plugin, string? iconName, string label, PluginImageSize size)
    {
        var layout = plugin.Client.LastLayout;
        var bg = Color(layout?["background"]?.ToString(), BitmapColor.Black);
        var fg = Color(layout?["foreground"]?.ToString(), BitmapColor.White);
        return Icon(plugin.Client, iconName) is { } icon ? Face(icon, size, bg) : Text(size, label, bg, fg);
    }

    /// <summary>A filled rounded rectangle from rectangles and corner circles (the SDK has none).</summary>
    private static void RoundedRect(BitmapBuilder b, int x, int y, int w, int h, int r, BitmapColor c)
    {
        r = Math.Max(0, Math.Min(r, Math.Min(w, h) / 2));
        if (r == 0) { b.FillRectangle(x, y, w, h, c); return; }
        b.FillRectangle(x, y + r, w, h - 2 * r, c);
        b.FillRectangle(x + r, y, w - 2 * r, h, c);
        b.FillCircle(x + r, y + r, r, c);
        b.FillCircle(x + w - r, y + r, r, c);
        b.FillCircle(x + r, y + h - r, r, c);
        b.FillCircle(x + w - r, y + h - r, r, c);
    }

    /// <summary>The key view for a parameter (<c>p2k5</c>, <c>k5</c>, <c>pad3</c>) in a layout.</summary>
    public static JsonNode? View(JsonNode? layout, string param)
    {
        if (layout == null) return null;
        if (param.StartsWith("pad") && int.TryParse(param.AsSpan(3), out var pad))
            return Find(layout["buttons"], $"pad.{pad}");
        if (param.StartsWith("ring") && int.TryParse(param.AsSpan(4), out var slot))
            return Find(layout["buttons"], $"ring.{slot}");
        if (param.StartsWith('p') && param.IndexOf('k') is int k and > 1
            && int.TryParse(param.AsSpan(1, k - 1), out var page) && int.TryParse(param.AsSpan(k + 1), out var key))
        {
            var pages = layout["keyPages"] as JsonArray;
            var p = pages != null && page >= 1 && page <= pages.Count ? pages[page - 1] : null;
            return Find(p?["keys"], $"key.{key}");
        }
        if (param.StartsWith('k') && int.TryParse(param.AsSpan(1), out var current))
            return Find(layout["keys"], $"key.{current}");
        return null;
    }

    private static JsonNode? Find(JsonNode? list, string control)
    {
        if (list is not JsonArray a) return null;
        foreach (var n in a)
            if (n?["control"]?.ToString() == control) return n;
        return null;
    }

    /// <summary>The key (1–9) that carries the one status hint while Chromaforge can't be used: the
    /// first key of the everyday layout's first page that does nothing, else key 1.</summary>
    public static int HintKey(SurfaceClient c)
    {
        var keys = (c.BaseLayout?["keyPages"] as JsonArray)?.FirstOrDefault()?["keys"] as JsonArray;
        if (keys != null)
            for (int i = 1; i <= 9; i++)
                if (Find(keys, $"key.{i}") is { } v && v["binding"] == null) return i;
        return 1;
    }
}

/// <summary>
/// Keys and buttons. Parameters: <c>p1k1</c>…<c>p3k9</c> (the Keypad's key 1–9 on its own page
/// 1–3), <c>k1</c>…<c>k9</c> (the current Chromaforge key page), <c>pad1</c>…<c>pad4</c> (the
/// Dialpad's buttons), <c>ring1</c>…<c>ring8</c> (the Actions Ring's slots). Presses and releases go to
/// Chromaforge; it decides what they do.
/// </summary>
public class ChromaforgeKey : PluginDynamicCommand
{
    // The parameterless base constructor makes every parameter an action of its own in Options+
    // (the display-name constructor registers one action without parameters, and profiles that
    // name a parameter then show a warning triangle instead of the key).
    private readonly List<string> _params = new();

    public ChromaforgeKey() : base()
    {
        Name = "ChromaforgeKey";
        DisplayName = "Chromaforge Key";
        Description = "A key whose action is set in Chromaforge (Settings, Controls)";
        for (int page = 1; page <= 3; page++)
            for (int k = 1; k <= 9; k++)
                Add($"p{page}k{k}", $"Keypad page {page}, key {k}");
        for (int k = 1; k <= 9; k++)
            Add($"k{k}", $"Key {k} of Chromaforge's current page");
        for (int pad = 1; pad <= 4; pad++)
            Add($"pad{pad}", $"Dialpad button {pad}");
        for (int slot = 1; slot <= 8; slot++)
            Add($"ring{slot}", $"Actions Ring slot {slot}");
    }

    private void Add(string name, string display)
    {
        AddParameter(name, display, Groups.Keys);
        _params.Add(name);
    }

    private ChromaforgePlugin? Owner => Plugin as ChromaforgePlugin;

    protected override bool OnLoad()
    {
        // the parameterless ActionImageChanged() refreshes only an action without parameters:
        // every key has to be named, or Options+ and the device keep their first image. Only the
        // keys whose look changed are redrawn: redrawing all 40 on every push (a turning dial
        // pushes one every few frames) kept the Logi service busy, and input waited behind it.
        if (Owner is { } p)
            p.Refresh += () =>
            {
                foreach (var n in _params)
                {
                    var (view, dim, hint) = Now(p, n);
                    if (_drawn.Changed(n, Redraw.Signature(p, view, dim, hint))) ActionImageChanged(n);
                }
            };
        return true;
    }

    private readonly Redraw _drawn = new();

    private static (string control, int? page)? Target(string param)
    {
        if (param.StartsWith("pad") && int.TryParse(param.AsSpan(3), out var pad)) return ($"pad.{pad}", null);
        if (param.StartsWith("ring") && int.TryParse(param.AsSpan(4), out var slot)) return ($"ring.{slot}", null);
        if (param.StartsWith('p') && param.IndexOf('k') is int k and > 1
            && int.TryParse(param.AsSpan(1, k - 1), out var page) && int.TryParse(param.AsSpan(k + 1), out var key))
            return ($"key.{key}", page);
        if (param.StartsWith('k') && int.TryParse(param.AsSpan(1), out var current)) return ($"key.{current}", null);
        return null;
    }

    protected override bool ProcessButtonEvent2(string actionParameter, DeviceButtonEvent2 buttonEvent)
    {
        if (Owner is not { } p || Target(actionParameter) is not { } t) return false;
        if (buttonEvent.EventType == DeviceButtonEventType.Press)
        {
            p.Client.Press(t.control, t.page);
            p.Haptic(Haptics.KeyPressed);
        }
        else if (buttonEvent.EventType == DeviceButtonEventType.Release)
        {
            p.Client.Release(t.control);
        }
        return true;
    }

    protected override void RunCommand(string actionParameter)
    {
        // devices that report only a press (no press / release events)
        if (Owner is not { } p || Target(actionParameter) is not { } t) return;
        p.Client.Press(t.control, t.page);
        p.Client.Release(t.control);
        p.Haptic(Haptics.KeyPressed);
    }

    /// <summary>What to show now: (view, dimmed, hint). Live: the layout. Not live: the everyday
    /// layout dimmed, and one key (<see cref="Renderer.HintKey"/>) says what to do.</summary>
    private static (JsonNode? view, bool dim, string? hint) Now(ChromaforgePlugin p, string param)
    {
        var c = p.Client;
        if (c.State == SurfaceState.Live) return (Renderer.View(c.Layout, param), false, null);
        var h = Renderer.HintKey(c);
        var hint = param == $"p1k{h}" || param == $"k{h}" ? p.Hint() : null;
        return (Renderer.View(c.BaseLayout, param), true, hint);
    }

    protected override string GetCommandDisplayName(string actionParameter, PluginImageSize imageSize)
    {
        if (Owner is not { } p) return " ";
        var (view, dim, hint) = Now(p, actionParameter);
        return hint != null ? p.BlankTitle : Renderer.Title(p, view, dim);
    }

    protected override BitmapImage GetCommandImage(string actionParameter, PluginImageSize imageSize)
    {
        if (Owner is not { } p) return new BitmapBuilder(imageSize).ToImage();
        var (view, dim, hint) = Now(p, actionParameter);
        if (hint != null)
        {
            var bg = Renderer.Color(p.Client.LastLayout?["background"]?.ToString(), BitmapColor.Black);
            return Renderer.Text(imageSize, hint, bg, new BitmapColor(255, 160, 60));
        }
        return Renderer.Key(p.Client, view, imageSize, dim);
    }
}

/// <summary>
/// The Dialpad's two encoders. The parameter names are those of the default profile
/// (<c>DefaultProfile71.lp5</c>), which binds Logi's encoder 0 to <c>dial</c> and encoder 1 to
/// <c>roller</c>; but on the MX Creative Dialpad encoder 0 is the ROLLER and encoder 1 the big
/// DIAL (TactileBridge's Wedding layout: Exposure on encoder 0, the roller). So parameter
/// <c>dial</c> drives Chromaforge's <c>roller</c> and parameter <c>roller</c> its <c>dial</c>; the
/// profiles already installed keep working.
/// </summary>
public class ChromaforgeDial : PluginDynamicAdjustment
{
    public ChromaforgeDial() : base(hasReset: true)
    {
        Name = "ChromaforgeDial";
        DisplayName = "Chromaforge Dial";
        Description = "A dial whose value is set in Chromaforge (Settings, Controls)";
        AddParameter("dial", "Dialpad roller (encoder 0)", Groups.Keys);
        AddParameter("roller", "Dialpad dial (encoder 1)", Groups.Keys);
    }

    private readonly Redraw _drawn = new();

    /// <summary>Chromaforge's control for a parameter (see the class comment).</summary>
    internal static string Control(string param) => param == "dial" ? "roller" : "dial";

    private ChromaforgePlugin? Owner => Plugin as ChromaforgePlugin;

    protected override bool OnLoad()
    {
        if (Owner is { } p)
            p.Refresh += () =>
            {
                var live = p.Client.State == SurfaceState.Live;
                foreach (var n in new[] { "dial", "roller" })
                {
                    var view = View(live ? p.Client.Layout : p.Client.BaseLayout, n);
                    if (!_drawn.Changed(n, Redraw.Signature(p, view, !live, live ? null : p.Hint()))) continue;
                    AdjustmentValueChanged(n);
                    ActionImageChanged(n);
                }
            };
        return true;
    }

    private static JsonNode? View(JsonNode? layout, string param)
    {
        if (layout?["dials"] is not JsonArray a) return null;
        var control = Control(param);
        foreach (var n in a)
            if (n?["control"]?.ToString() == control) return n;
        return null;
    }

    protected override void ApplyAdjustment(string actionParameter, int diff)
    {
        if (Owner is not { } p || diff == 0) return;
        p.Client.Turn(Control(actionParameter), diff);
        var pos = View(p.Client.Layout, actionParameter)?["position"]?.GetValue<double>();
        if (pos is <= 0.0 or >= 1.0) p.Haptic(Haptics.AtLimit);
    }

    protected override void RunCommand(string actionParameter)
    {
        // an encoder press: Chromaforge resets the value it moves (or runs `dial.press`)
        if (Owner is not { } p) return;
        var control = Control(actionParameter);
        p.Client.Press(control);
        p.Client.Release(control);
        p.Haptic(Haptics.ValueReset);
    }

    protected override string GetAdjustmentValue(string actionParameter)
    {
        if (Owner is not { } p) return "";
        return View(p.Client.Layout, actionParameter)?["value"]?.ToString() ?? "";
    }

    protected override string GetAdjustmentDisplayName(string actionParameter, PluginImageSize imageSize)
    {
        if (Owner is not { } p) return "";
        var live = p.Client.State == SurfaceState.Live;
        var label = View(live ? p.Client.Layout : p.Client.BaseLayout, actionParameter)?["label"]?.ToString();
        return string.IsNullOrEmpty(label) ? (live ? p.BlankTitle : p.Hint().Replace('\n', ' ')) : label;
    }

    protected override BitmapImage GetAdjustmentImage(string actionParameter, PluginImageSize imageSize)
    {
        if (Owner is not { } p) return new BitmapBuilder(imageSize).ToImage();
        var live = p.Client.State == SurfaceState.Live;
        var view = View(live ? p.Client.Layout : p.Client.BaseLayout, actionParameter);
        return Renderer.Key(p.Client, view, imageSize, !live);
    }
}

/// <summary>
/// What each key was last drawn from, so a layout push redraws only the keys whose look changed
/// (a turning dial changes one or two). Thread-safe.
/// </summary>
internal sealed class Redraw
{
    private readonly Dictionary<string, string> _last = new(StringComparer.Ordinal);

    /// <summary>True (and remembered) when <paramref name="signature"/> differs from last time.</summary>
    public bool Changed(string param, string signature)
    {
        lock (_last)
        {
            if (_last.TryGetValue(param, out var old) && old == signature) return false;
            _last[param] = signature;
            return true;
        }
    }

    /// <summary>Everything a key's image and title are drawn from.</summary>
    public static string Signature(ChromaforgePlugin p, JsonNode? view, bool dim, string? hint)
    {
        var layout = p.Client.LastLayout;
        return string.Join("\u0001", view?.ToJsonString() ?? "", dim ? "1" : "0", hint ?? "", p.BlankTitle, Renderer.Contrast,
            p.Client.IconsVersion.ToString(), layout?["background"]?.ToString() ?? "", layout?["foreground"]?.ToString() ?? "");
    }
}
