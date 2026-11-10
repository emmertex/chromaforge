// Console input waiting to go to Chromaforge: dial ticks merged, repeated presses dropped.
// Copyright (c) 2026 RAD Films - Roman Alurkoff. MIT License.
// Portions from TactileBridge (the author's Capture One plugin, CaptureOneAdjustment.cs): a dial
// that reverses takes the new direction at once instead of finishing the old one.

using System.Text.Json.Nodes;

namespace Chromaforge.Surface.Client;

/// <summary>
/// The input events not sent yet, in order. The client sends them as ONE <c>surface.input</c>
/// request and only while no other is waiting for its reply, so when Chromaforge is slow the
/// events pile up here, merged, instead of as hundreds of requests that Chromaforge then replays
/// in a burst ("the knob freezes, then clicks itself five times"):
/// <list type="bullet">
/// <item>ticks of the same dial add up into one event (unless a press came in between, which
///   keeps its place);</item>
/// <item>a dial that reverses drops the ticks of the old direction not sent yet and goes the new
///   way at once (TactileBridge's direction change);</item>
/// <item>a key pressed again while its earlier press still waits here is one press, not two.</item>
/// </list>
/// Thread-safe.
/// </summary>
public sealed class InputQueue
{
    private sealed class Ev
    {
        public required string Kind; // "control" or "action"
        public required string Name;
        public int? Ticks;
        public bool? Press;
        public int? Page;
    }

    private readonly object _lock = new();
    private readonly List<Ev> _events = new();
    /// Keys whose repeated press was dropped: their next release is dropped too.
    private readonly HashSet<string> _swallowRelease = new(StringComparer.Ordinal);

    /// <summary>The most events one request carries (Chromaforge takes at most 256).</summary>
    public const int MaxBatch = 200;

    /// <summary>The most events kept waiting; beyond it the oldest presses give way.</summary>
    public const int MaxQueued = 1000;

    public int Count { get { lock (_lock) return _events.Count; } }

    /// <summary>A dial or encoder turned (<paramref name="kind"/>: <c>control</c> or <c>action</c>).</summary>
    public void Turn(string kind, string name, int ticks)
    {
        if (ticks == 0) return;
        lock (_lock)
        {
            // the last event for this dial, if no press or release came after it
            for (int i = _events.Count - 1; i >= 0; i--)
            {
                var e = _events[i];
                if (e.Ticks == null) break;
                if (e.Kind != kind || e.Name != name) continue;
                int had = e.Ticks.Value;
                if (Math.Sign(had) != Math.Sign(ticks))
                    e.Ticks = ticks; // reversed: the old direction's unsent ticks are dropped
                else
                    e.Ticks = (int)Math.Clamp((long)had + ticks, int.MinValue / 2, int.MaxValue / 2);
                return;
            }
            Add(new Ev { Kind = kind, Name = name, Ticks = ticks });
        }
    }

    /// <summary>A key went down or up.</summary>
    public void Button(string kind, string name, bool down, int? page = null)
    {
        lock (_lock)
        {
            var key = kind + ":" + name;
            if (down)
            {
                // already pressed and still waiting here: one press is enough
                if (_events.Any(e => e.Press == true && e.Kind == kind && e.Name == name))
                {
                    _swallowRelease.Add(key);
                    return;
                }
            }
            else if (_swallowRelease.Remove(key))
            {
                return;
            }
            Add(new Ev { Kind = kind, Name = name, Press = down, Page = page });
        }
    }

    private void Add(Ev e)
    {
        _events.Add(e);
        if (_events.Count > MaxQueued) _events.RemoveAt(0);
    }

    /// <summary>Take up to <see cref="MaxBatch"/> events, oldest first, as protocol objects.</summary>
    public JsonArray? Take()
    {
        lock (_lock)
        {
            if (_events.Count == 0) return null;
            int n = Math.Min(MaxBatch, _events.Count);
            var arr = new JsonArray();
            foreach (var e in _events.Take(n))
            {
                var o = new JsonObject { [e.Kind] = e.Name };
                if (e.Ticks is int t) o["ticks"] = t;
                if (e.Press is bool p) o["press"] = p;
                if (e.Page is int pg) o["page"] = pg;
                arr.Add(o);
            }
            _events.RemoveRange(0, n);
            return arr;
        }
    }

    /// <summary>Forget everything waiting (the connection dropped: stale input never replays).</summary>
    public void Clear()
    {
        lock (_lock)
        {
            _events.Clear();
            _swallowRelease.Clear();
        }
    }
}
