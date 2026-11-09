// Tests of the surface client's input path, without LightCraft or Logi: the input queue's merging
// rules, and the client against a fake LightCraft that answers slowly (a stall), checking input
// never piles up into hundreds of requests that replay later.
// Copyright (c) 2026 RAD Films - Roman Alurkoff. MIT License.
//
//   dotnet run --project tools/ClientTests -c Release

using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Text.Json.Nodes;
using LightCraft.Surface.Client;

int failures = 0;
void Check(bool ok, string what) { Console.WriteLine($"{(ok ? "PASS" : "FAIL")}  {what}"); if (!ok) failures++; }

// ------------------------------------------------------------------ the queue alone
{
    var q = new InputQueue();
    for (int i = 0; i < 500; i++) q.Turn("control", "dial", 1);
    var b = q.Take();
    Check(b?.Count == 1 && b[0]?["ticks"]?.GetValue<int>() == 500, "500 ticks of one dial merge into one event of 500");

    q.Turn("control", "dial", 5);
    q.Turn("control", "dial", -1);
    q.Turn("control", "dial", -1);
    b = q.Take();
    Check(b?.Count == 1 && b[0]?["ticks"]?.GetValue<int>() == -2, "a reversal drops the old direction's unsent ticks (TactileBridge): +5 -1 -1 → -2");

    q.Turn("control", "dial", 3);
    q.Button("control", "key.1", true);
    q.Button("control", "key.1", false);
    q.Turn("control", "dial", 2);
    b = q.Take();
    Check(b?.Count == 4 && b[0]?["ticks"]?.GetValue<int>() == 3 && b[3]?["ticks"]?.GetValue<int>() == 2,
        "a press between turns keeps its place (turn 3, press, release, turn 2)");

    for (int i = 0; i < 5; i++) { q.Button("control", "key.2", true, 1); q.Button("control", "key.2", false); }
    b = q.Take();
    Check(b?.Count == 2 && b[0]?["press"]?.GetValue<bool>() == true && b[0]?["page"]?.GetValue<int>() == 1 && b[1]?["press"]?.GetValue<bool>() == false,
        "five presses of a key while the first still waits are one press and one release");

    q.Turn("control", "dial", 1);
    q.Turn("control", "roller", 1);
    q.Turn("action", "v.light.exposure", 2);
    q.Turn("control", "dial", 1);
    b = q.Take();
    Check(b?.Count == 3 && b[0]?["ticks"]?.GetValue<int>() == 2 && b[2]?["action"]?.ToString() == "v.light.exposure",
        "each dial and action merges on its own");

    for (int i = 0; i < 2000; i++) { q.Button("control", $"key.{i}", true); }
    Check(q.Count == InputQueue.MaxQueued, $"the queue is bounded ({q.Count})");
    Check(q.Take()?.Count == InputQueue.MaxBatch, "one request carries at most MaxBatch events");
    q.Clear();
    Check(q.Count == 0 && q.Take() == null, "Clear forgets everything");
}

// ------------------------------------------------------------------ the client against a slow LightCraft
{
    var server = new FakeLightCraft(replyDelayMs: 0);
    var client = new SurfaceClient("127.0.0.1", server.Port, "tests", "Tests", "mx-creative-console", new MemTokens("t"), null);
    client.Start();
    Check(Wait(() => client.State == SurfaceState.Live, 5000), "connects to the fake LightCraft");

    // fast: 300 ticks in a burst
    for (int i = 0; i < 300; i++) client.Turn("dial", 1);
    Check(Wait(() => server.Ticks("dial") == 300, 3000), $"a fast LightCraft gets every tick ({server.Ticks("dial")} of 300)");
    Check(server.InputRequests < 300, $"in fewer requests than ticks ({server.InputRequests})");

    // a stall: each reply takes 400 ms; 1000 ticks spread over a second, plus five presses
    server.ReplyDelayMs = 400;
    long before = server.InputRequests;
    var sw = Stopwatch.StartNew();
    for (int i = 0; i < 1000; i++)
    {
        client.Turn("dial", 1);
        if (i % 10 == 0) Thread.Sleep(10);
    }
    // five quick presses while a request is still waiting for its reply
    for (int i = 0; i < 5; i++) { client.Press("key.3"); client.Release("key.3"); Thread.Sleep(5); }
    Check(Wait(() => server.Ticks("dial") == 1300, 6000), $"a stalled LightCraft still gets the sum of the ticks, once ({server.Ticks("dial")} of 1300)");
    long sent = server.InputRequests - before;
    Check(sent <= 12, $"one request in flight while it stalls: {sent} requests for 1000 ticks over {sw.ElapsedMilliseconds} ms (not 1000 queued)");
    Check(Wait(() => server.Presses("key.3") >= 1, 3000) && server.Presses("key.3") == 1, $"presses during the stall are not replayed in a burst ({server.Presses("key.3")} of 5 sent)");
    Check(Wait(() => client.QueuedInput == 0, 2000), "nothing left queued afterwards");

    // the reply never comes: the next batch goes anyway after the in-flight limit
    server.ReplyDelayMs = -1;
    client.Turn("roller", 4);
    Thread.Sleep(50);
    client.Turn("roller", 4);
    Check(Wait(() => server.Ticks("roller") == 8, 5000), "a lost reply never stalls the console (the next batch goes after the limit)");

    client.Dispose();
    server.Stop();
}

// ------------------------------------------------------------------ the Loupedeck+ profile repair
{
    var repo = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "..", "..", "..", "..", ".."));
    var profiles = Path.Combine(repo, "src", "LightCraftPlugin", "profiles");
    var defaults = LightCraft.Logi.ProfileRepair.DefaultSlots(Path.Combine(profiles, "DefaultProfile15.lp5"));
    Check(defaults != null && defaults["pressActions"][33] == "$LightCraft___LoupedeckPlusButton___b33" && defaults["rotateActions"][1] == "$LightCraft___LoupedeckPlusKnob___r1",
        "reads the Loupedeck+ default profile's pass-through actions");

    // a profile the service made before the plugin had a Loupedeck+ default: empty, except a knob
    // assigned by hand to a LightCraft value (the workaround) and a button with a keyboard shortcut
    var tmp = Path.Combine(Path.GetTempPath(), "lc-profile-repair-" + Guid.NewGuid().ToString("N"));
    var apps = Path.Combine(tmp, "Applications");
    var dir = Path.Combine(apps, "Loupedeck15", "@_lightcraft", "Profiles", "ABC");
    Directory.CreateDirectory(dir);
    JsonArray Nulls(int n) { var a = new JsonArray(); for (int i = 0; i < n; i++) a.Add(null); return a; }
    var ws = new JsonObject { ["$type"] = "Workspace", ["name"] = "W", ["pressActions"] = Nulls(128), ["rotateActions"] = Nulls(128) };
    ws["rotateActions"]![1] = "$LightCraft___LightCraftAdjust___v.light.exposure";
    ws["pressActions"]![33] = "$LightCraft___ResetLightCraftAdjust___v.light.exposure";
    ws["pressActions"]![95] = "$DefaultWin___KeyboardKey___Ctrl+Z";
    var profile = new JsonObject
    {
        ["$type"] = "Loupedeck.Service.ApplicationProfile, LoupedeckService",
        ["layout"] = new JsonObject { ["layoutModes"] = new JsonArray(new JsonObject { ["workspaces"] = new JsonArray(ws, ws.DeepClone()) }) },
    };
    File.WriteAllText(Path.Combine(dir, "ProfileInfo.json"), profile.ToJsonString());
    var backups = Path.Combine(tmp, "backups");
    var done = LightCraft.Logi.ProfileRepair.Run(apps, profiles, backups, null);
    var after = JsonNode.Parse(File.ReadAllText(Path.Combine(dir, "ProfileInfo.json")));
    var w0 = after?["layout"]?["layoutModes"]?[0]?["workspaces"]?[0];
    var w1 = after?["layout"]?["layoutModes"]?[0]?["workspaces"]?[1];
    Check(done.Count == 1, $"the empty profile is repaired ({done.FirstOrDefault()})");
    Check(w0?["rotateActions"]?[1]?.ToString() == "$LightCraft___LoupedeckPlusKnob___r1" && w0?["pressActions"]?[33]?.ToString() == "$LightCraft___LoupedeckPlusButton___b33",
        "every control gets the pass-through, also the knob assigned to a LightCraft value by hand");
    Check(w0?["pressActions"]?[95]?.ToString() == "$DefaultWin___KeyboardKey___Ctrl+Z", "a control with another action (a keyboard shortcut) is left alone");
    Check(w1?["rotateActions"]?[48]?.ToString() == "$LightCraft___LoupedeckPlusKnob___r48", "in every workspace");
    Check(after?["$type"]?.ToString() == "Loupedeck.Service.ApplicationProfile, LoupedeckService", "the rest of the file is kept");
    Check(Directory.Exists(backups) && Directory.GetFiles(backups).Length == 1 && File.ReadAllText(Directory.GetFiles(backups)[0]).Contains("ResetLightCraftAdjust"),
        "the original is backed up first");
    Check(LightCraft.Logi.ProfileRepair.Run(apps, profiles, backups, null).Count == 0 && Directory.GetFiles(backups).Length == 1, "a second run changes nothing");

    // the user's real profile, copied (never touched in place)
    var real = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Logi", "LogiPluginService", "Applications", "Loupedeck15", "@_lightcraft", "Profiles");
    var realFile = Directory.Exists(real) ? Directory.EnumerateFiles(real, "ProfileInfo.json", SearchOption.AllDirectories).FirstOrDefault() : null;
    if (realFile != null)
    {
        var text = File.ReadAllText(realFile);
        var (fixedText, changed) = LightCraft.Logi.ProfileRepair.Repair(text, defaults!);
        var n = fixedText == null ? 0 : fixedText.Split("$LightCraft___LoupedeckPlus").Length - 1;
        Console.WriteLine($"      this PC's Loupedeck+ LightCraft profile (a copy): {changed} controls would change; pass-through slots after: {n}");
        Check(fixedText == null || JsonNode.Parse(fixedText) != null, "the repaired copy of this PC's profile is valid JSON");
    }
    try { Directory.Delete(tmp, true); } catch (Exception) { }
}

// ------------------------------------------------------------------ the Actions Ring profile repair
{
    var repo = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "..", "..", "..", "..", ".."));
    var profiles = Path.Combine(repo, "src", "LightCraftPlugin", "profiles");
    var defaults = LightCraft.Logi.ProfileRepair.DefaultSlots(Path.Combine(profiles, "DefaultProfile72.lp5"));
    Check(defaults != null && defaults.TryGetValue("pressPages/0/pressAction", out var ringSlots) && ringSlots[0] == "$LightCraft___LightCraftKey___ring1" && ringSlots[7] == "$LightCraft___LightCraftKey___ring8",
        "reads the Actions Ring default profile's eight slots");
    // the empty ring profile the service made: a page of eight controls with no action, and a
    // dial page; one slot set by hand to another plugin's action
    JsonObject Ctl(int id, string? press) => new() { ["$type"] = "Control7", ["controlId"] = id, ["pressAction"] = press, ["rotateAction"] = null };
    var page = new JsonArray(Enumerable.Range(0, 8).Select(i => (JsonNode?)Ctl(i, i == 5 ? "$DefaultWin___KeyboardKey___F5" : null)).ToArray());
    var ws = new JsonObject
    {
        ["pressPages"] = new JsonArray(new JsonObject { ["displayName"] = "Page (1)", ["controls"] = page }),
        ["rotatePages"] = new JsonArray(new JsonObject { ["displayName"] = "Dial Page", ["controls"] = new JsonArray(Ctl(0, null)) }),
    };
    var profile = new JsonObject { ["layout"] = new JsonObject { ["layoutModes"] = new JsonArray(new JsonObject { ["workspaces"] = new JsonArray(ws) }) } };
    var (text, changed) = LightCraft.Logi.ProfileRepair.Repair(profile.ToJsonString(), defaults!);
    var after = JsonNode.Parse(text ?? "{}")?["layout"]?["layoutModes"]?[0]?["workspaces"]?[0]?["pressPages"]?[0]?["controls"];
    Check(changed == 7, $"seven empty ring slots get the pass-through ({changed})");
    Check(after?[0]?["pressAction"]?.ToString() == "$LightCraft___LightCraftKey___ring1" && after?[7]?["pressAction"]?.ToString() == "$LightCraft___LightCraftKey___ring8",
        "slot 1 → ring1 … slot 8 → ring8");
    Check(after?[5]?["pressAction"]?.ToString() == "$DefaultWin___KeyboardKey___F5", "a slot with another action is left alone");

    var real = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Logi", "LogiPluginService", "Applications", "Loupedeck72", "@_lightcraft", "Profiles");
    var realFile = Directory.Exists(real) ? Directory.EnumerateFiles(real, "ProfileInfo.json", SearchOption.AllDirectories).FirstOrDefault() : null;
    if (realFile != null)
    {
        var (fixedText, n) = LightCraft.Logi.ProfileRepair.Repair(File.ReadAllText(realFile), defaults!);
        Console.WriteLine($"      this PC's Actions Ring LightCraft profile (a copy): {n} slots would change");
        Check(fixedText == null || JsonNode.Parse(fixedText) != null, "the repaired copy of this PC's ring profile is valid JSON");
    }
}

Console.WriteLine(failures == 0 ? "\nall passed" : $"\n{failures} FAILED");
return failures == 0 ? 0 : 1;

static bool Wait(Func<bool> cond, int ms)
{
    var sw = Stopwatch.StartNew();
    while (sw.ElapsedMilliseconds < ms) { if (cond()) return true; Thread.Sleep(10); }
    return cond();
}

sealed class MemTokens(string? token) : ITokenStore
{
    private string? _t = token;
    public string? Load() => _t;
    public void Save(string? t) => _t = t;
}

/// <summary>Speaks just enough of the surface protocol: hello (paired), catalog, icons, subscribe,
/// watch, and input (counting ticks and presses; replies after a delay, or never when -1).</summary>
sealed class FakeLightCraft
{
    private readonly TcpListener _l = new(IPAddress.Loopback, 0);
    private readonly Dictionary<string, long> _ticks = new();
    private readonly Dictionary<string, int> _presses = new();
    private volatile bool _stop;
    private long _inputRequests;
    public volatile int ReplyDelayMs;
    public int Port { get; }
    public long InputRequests => Interlocked.Read(ref _inputRequests);

    public FakeLightCraft(int replyDelayMs)
    {
        ReplyDelayMs = replyDelayMs;
        _l.Start();
        Port = ((IPEndPoint)_l.LocalEndpoint).Port;
        new Thread(Accept) { IsBackground = true }.Start();
    }

    public long Ticks(string c) { lock (_ticks) return _ticks.GetValueOrDefault(c); }
    public int Presses(string c) { lock (_ticks) return _presses.GetValueOrDefault(c); }
    public void Stop() { _stop = true; _l.Stop(); }

    private void Accept()
    {
        try
        {
            while (!_stop)
            {
                var c = _l.AcceptTcpClient();
                new Thread(() => Serve(c)) { IsBackground = true }.Start();
            }
        }
        catch (Exception) { }
    }

    private void Serve(TcpClient c)
    {
        try
        {
            var s = c.GetStream();
            var r = new StreamReader(s, new UTF8Encoding(false));
            var w = new object();
            void Send(JsonObject o)
            {
                var b = Encoding.UTF8.GetBytes(o.ToJsonString() + "\n");
                lock (w) s.Write(b, 0, b.Length);
            }
            while (!_stop && r.ReadLine() is { } line)
            {
                var m = JsonNode.Parse(line);
                var id = m?["id"]?.GetValue<long>() ?? 0;
                var method = m?["method"]?.ToString();
                JsonNode result = method switch
                {
                    "surface.hello" => new JsonObject { ["paired"] = true },
                    "surface.catalog" => new JsonObject { ["groups"] = new JsonArray(), ["entries"] = new JsonArray() },
                    "surface.icons" => new JsonObject(),
                    "surface.subscribe" => new JsonObject { ["layout"] = new JsonObject { ["context"] = "develop", ["keys"] = new JsonArray() } },
                    _ => new JsonObject(),
                };
                if (method == "surface.input")
                {
                    Interlocked.Increment(ref _inputRequests);
                    foreach (var e in m?["params"]?["events"]?.AsArray() ?? new JsonArray())
                    {
                        var name = e?["control"]?.ToString() ?? e?["action"]?.ToString() ?? "";
                        lock (_ticks)
                        {
                            if (e?["ticks"] is JsonNode t) _ticks[name] = _ticks.GetValueOrDefault(name) + t.GetValue<int>();
                            if (e?["press"]?.GetValue<bool>() == true) _presses[name] = _presses.GetValueOrDefault(name) + 1;
                        }
                    }
                    var delay = ReplyDelayMs;
                    if (delay < 0) continue; // never answers
                    if (delay > 0) Thread.Sleep(delay); // LightCraft busy: this connection waits
                }
                Send(new JsonObject { ["id"] = id, ["ok"] = true, ["result"] = result });
            }
        }
        catch (Exception) { }
    }
}
