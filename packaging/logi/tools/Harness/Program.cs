// Test harness: drives a running Chromaforge (started with --control PORT) through the same
// protocol client the plugin uses, and checks what happens in the app.
// Copyright (c) 2026 RAD Films - Roman Alurkoff. MIT License.
//
//   dotnet run --project tools/Harness -- 18532
//
// 1. pairs (approving the request through the control channel, as a user would click Approve),
// 2. subscribes and checks the pushed layout,
// 3. turns the dial and checks Exposure changed and the new value was pushed,
// 4. presses a key and checks it ran its command,
// 5. forgets the pairing again.

using System.Net.Sockets;
using System.Text;
using System.Text.Json.Nodes;
using Chromaforge.Surface.Client;

int port = args.Length > 0 && int.TryParse(args[0], out var p) ? p : 7980;
const string ClientId = "chromaforge-logi-harness";
int failures = 0;
void Check(bool ok, string what)
{
    Console.WriteLine($"{(ok ? "PASS" : "FAIL")}  {what}");
    if (!ok) failures++;
}

// a plain control connection (the app's full --control port) to watch and to approve
using var ctl = new TcpClient("127.0.0.1", port);
var ctlStream = ctl.GetStream();
var ctlReader = new StreamReader(ctlStream, new UTF8Encoding(false));
long nextId = 1000;
JsonNode? Ctl(string method, JsonObject? prm = null)
{
    var id = ++nextId;
    var line = new JsonObject { ["id"] = id, ["method"] = method, ["params"] = prm ?? new JsonObject() }.ToJsonString() + "\n";
    var b = Encoding.UTF8.GetBytes(line);
    ctlStream.Write(b);
    while (true)
    {
        var l = ctlReader.ReadLine();
        if (l == null) return null;
        var v = JsonNode.Parse(l);
        if (v?["id"]?.GetValue<long>() == id) return v;
    }
}
JsonNode? Exec(string command, JsonObject? prm = null) =>
    Ctl("engine.execute", new JsonObject { ["command"] = command, ["params"] = prm ?? new JsonObject() });

var state = Exec("surface.state");
Check(state?["ok"]?.GetValue<bool>() == true, "Chromaforge answers on the control port");
if (state?["result"]?["enabled"]?.GetValue<bool>() != true)
{
    Console.WriteLine("Control surfaces are off in this Chromaforge: turn them on in Settings, Controls.");
    return 2;
}
// a photo open in Detail
if (Exec("library.state")?["result"]?["selection"]?["active"] == null)
    Exec("library.next");
Exec("view.detail");
double Exposure() => Exec("photo.inspect")?["result"]?["develop"]?["light"]?["exposure"]?.GetValue<double>() ?? double.NaN;
long? Active() => Exec("library.state")?["result"]?["selection"]?["active"]?.GetValue<long>();

var tokenFile = Path.Combine(Path.GetTempPath(), $"chromaforge-harness-token-{Environment.ProcessId}.txt");
using var client = new SurfaceClient("127.0.0.1", port, ClientId, "Chromaforge harness", "mx-creative-console", new FileTokenStore(tokenFile), s => Console.WriteLine($"      client: {s}"));
var layouts = 0;
client.LayoutChanged += () => Interlocked.Increment(ref layouts);
client.Start();

// 1. pairing: wait for the request, approve it the way the Approve button does
var t0 = DateTime.UtcNow;
while (client.State != SurfaceState.Pairing && client.State != SurfaceState.Live && DateTime.UtcNow - t0 < TimeSpan.FromSeconds(20)) Thread.Sleep(100);
Check(client.State == SurfaceState.Pairing && client.PairCode?.Length == 6, $"the client asks to pair (code {client.PairCode})");
Exec("app.settings", new JsonObject { ["tab"] = "controls" });
var shown = Exec("surface.state")?["result"]?["pairing"]?.AsArray().Any(r => r?["code"]?.ToString() == client.PairCode) == true;
Check(shown, "Chromaforge lists the request with the same code");
bool IsPaired() => Exec("surface.state")?["result"]?["paired"]?.AsArray().Any(c => c?["client"]?.ToString() == ClientId) == true;
bool Waiting() => Exec("surface.state")?["result"]?["pairing"]?.AsArray().Any(r => r?["code"]?.ToString() == client.PairCode) == true;
// the dialog fades in for a few frames: click Approve until the request is answered
for (int i = 0; i < 10 && Waiting(); i++)
{
    Thread.Sleep(400);
    Ctl("ui.clickWidget", new JsonObject { ["id"] = $"button:surfaceApprove-{ClientId}" });
}
Check(IsPaired(), "Approve clicked in Settings, Controls");
Ctl("ui.dialog.cancel");
t0 = DateTime.UtcNow;
while (client.State != SurfaceState.Live && DateTime.UtcNow - t0 < TimeSpan.FromSeconds(20)) Thread.Sleep(100);
Check(client.State == SurfaceState.Live, "the client is paired and subscribed");

// 2. the layout
var layout = client.Layout;
Check(layout?["keys"]?.AsArray().Count == 9 && layout?["dials"]?.AsArray().Count == 2, "the layout has 9 keys and 2 dials");
Check(layout?["keyPages"]?.AsArray().Count >= 3, "three key pages for the Keypad's own pages");

// 3. the dial, in the Default profile on its first dial page
var profileBefore = Exec("surface.state")?["result"]?["profile"]?.ToString();
Exec("surface.profile", new JsonObject { ["profile"] = "Default" });
Exec("surface.page", new JsonObject { ["page"] = "Dial Page" });
Thread.Sleep(1200);
layout = client.Layout;
// whichever encoder moves Exposure on this page (the user may have rearranged the profile)
var exposureDial = layout?["dials"]?.AsArray().FirstOrDefault(d => d?["binding"]?["target"]?.ToString() == "light.exposure");
var dialControl = exposureDial?["control"]?.ToString() ?? "dial";
var dialTarget = exposureDial?["binding"]?["target"]?.ToString();
var before = Exposure();
var pushesBefore = layouts;
client.Turn(dialControl, 10);
Thread.Sleep(1500);
var after = Exposure();
Check(dialTarget == "light.exposure", $"the {dialControl} moves {dialTarget}");
// expected: 10 ticks × 0.01 × speed (global × Exposure's own) × the slow-turning floor
var settings = Exec("surface.settings")?["result"];
double speed = (settings?["motion"]?["speed"]?.GetValue<double>() ?? 1.0) * (settings?["tuning"]?["light.exposure"]?["speed"]?.GetValue<double>() ?? 1.0);
double floor = settings?["tuning"]?["light.exposure"]?["accelerationMin"]?.GetValue<double>() ?? 1.0;
double expected = 10 * 0.01 * Math.Clamp(speed, 0.1, 5.0) * floor;
Check(Math.Abs(after - before - expected) < 0.006, $"10 dial ticks: exposure {before:+0.00;-0.00} → {after:+0.00;-0.00} (expected +{expected:0.000})");
var pushed = client.Layout?["dials"]?.AsArray().FirstOrDefault(d => d?["control"]?.ToString() == dialControl)?["value"]?.ToString();
Check(layouts > pushesBefore && pushed == after.ToString("+0.00;-0.00;0.00", System.Globalization.CultureInfo.InvariantCulture), $"the new value was pushed ({pushed})");
Exec("edit.undo");
Check(Math.Abs(Exposure() - before) < 1e-6, "one undo puts the whole turn back");

// 4. a key: the Default profile's Keypad page 1, key 2 = Next photo
var activeBefore = Active();
client.Press("key.2", 1);
client.Release("key.2");
Thread.Sleep(800);
Check(Active() != activeBefore, "page 1 key 2 moved to the next photo");
client.Press("key.1", 1);
client.Release("key.1");
Thread.Sleep(500);
if (profileBefore != null) Exec("surface.profile", new JsonObject { ["profile"] = profileBefore });

// 5. forget the harness again (the real plugin keeps its own pairing)
Check(IsPaired(), "Chromaforge lists the harness as paired");
Exec("app.settings", new JsonObject { ["tab"] = "controls" });
for (int i = 0; i < 10 && IsPaired(); i++)
{
    Thread.Sleep(400);
    // Forget sits under Profiles and Setup
    Ctl("ui.clickWidget", new JsonObject { ["id"] = "button:surfaceSection-setup" });
    Thread.Sleep(200);
    Ctl("ui.clickWidget", new JsonObject { ["id"] = $"button:surfaceForget-{ClientId}" });
}
// Forget may be scrolled out of view in the Settings window: the command behind it
if (IsPaired()) Exec("surface.forget", new JsonObject { ["client"] = ClientId });
Check(!IsPaired(), "Forget removes the pairing");
Ctl("ui.dialog.cancel");
try { File.Delete(tokenFile); } catch { }
Console.WriteLine(failures == 0 ? "\nAll checks passed." : $"\n{failures} check(s) failed.");
return failures == 0 ? 0 : 1;
