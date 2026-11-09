// The Loupedeck+ console: its knobs and buttons forwarded to LightCraft by Loupedeck's own numbers.
// Copyright (c) 2026 RAD Films - Roman Alurkoff. MIT License.
//
// The default profile (DefaultProfile15.lp5) puts each of these on the control with the same
// number; LightCraft decides what it does (Settings, Controls, Loupedeck+): by default what the
// console's label says, or the layout imported from the user's own Loupedeck profile.

using Loupedeck;

namespace LightCraft.Logi;

internal static class LoupedeckPlusControls
{
    public const string Group = "1. Loupedeck+ (set in LightCraft)";

    /// <summary>The turns Loupedeck numbers: P1-P8 in their four modes (1-32), the labelled knobs
    /// (33-47, no 43) and the Control Dial (48).</summary>
    public static IEnumerable<int> Knobs() => Enumerable.Range(1, 48).Where(n => n != 43);

    /// <summary>The presses: the knobs and the Control Dial (33-48), the buttons (49-54, 66-104),
    /// Custom Mode (110) and L1-L3 (114-116).</summary>
    public static IEnumerable<int> Buttons() =>
        Enumerable.Range(33, 16).Where(n => n != 43)
            .Concat(Enumerable.Range(49, 6))
            .Concat(Enumerable.Range(66, 39))
            .Append(110)
            .Concat(Enumerable.Range(114, 3));

    public static int? Number(string param, char prefix) =>
        param.Length > 1 && param[0] == prefix && int.TryParse(param.AsSpan(1), out var n) && n is > 0 and < 128 ? n : null;
}

/// <summary>A Loupedeck+ button (or a knob's press): <c>b95</c> is LightCraft's <c>ld.b95</c>.</summary>
public class LoupedeckPlusButton : PluginDynamicCommand
{
    public LoupedeckPlusButton() : base()
    {
        Name = "LoupedeckPlusButton";
        DisplayName = "LightCraft: Loupedeck+ button";
        Description = "A Loupedeck+ button whose action is set in LightCraft (Settings, Controls, Loupedeck+)";
        foreach (var n in LoupedeckPlusControls.Buttons())
            AddParameter($"b{n}", $"Loupedeck+ button {n}", LoupedeckPlusControls.Group);
    }

    private LightCraftPlugin? Owner => Plugin as LightCraftPlugin;

    protected override bool ProcessButtonEvent2(string actionParameter, DeviceButtonEvent2 buttonEvent)
    {
        if (Owner is not { } p || LoupedeckPlusControls.Number(actionParameter, 'b') is not { } n) return false;
        if (buttonEvent.EventType == DeviceButtonEventType.Press) p.Client.Press($"ld.b{n}");
        else if (buttonEvent.EventType == DeviceButtonEventType.Release) p.Client.Release($"ld.b{n}");
        return true;
    }

    protected override void RunCommand(string actionParameter)
    {
        if (Owner is not { } p || LoupedeckPlusControls.Number(actionParameter, 'b') is not { } n) return;
        p.Client.Press($"ld.b{n}");
        p.Client.Release($"ld.b{n}");
    }
}

/// <summary>A Loupedeck+ knob: <c>r33</c> is LightCraft's <c>ld.r33</c>.</summary>
public class LoupedeckPlusKnob : PluginDynamicAdjustment
{
    public LoupedeckPlusKnob() : base(hasReset: false)
    {
        Name = "LoupedeckPlusKnob";
        DisplayName = "LightCraft: Loupedeck+ knob";
        Description = "A Loupedeck+ knob whose value is set in LightCraft (Settings, Controls, Loupedeck+)";
        foreach (var n in LoupedeckPlusControls.Knobs())
            AddParameter($"r{n}", $"Loupedeck+ knob {n}", LoupedeckPlusControls.Group);
    }

    private LightCraftPlugin? Owner => Plugin as LightCraftPlugin;

    protected override void ApplyAdjustment(string actionParameter, int diff)
    {
        if (Owner is not { } p || diff == 0 || LoupedeckPlusControls.Number(actionParameter, 'r') is not { } n) return;
        p.Client.Turn($"ld.r{n}", diff);
    }
}
