// Zero-setup check: Chromaforge trusts the plugin it finds installed by writing a token and the
// port into the plugin's data folder. This connects exactly as the plugin does, from that folder,
// and checks it is live without any pairing, and that the keys follow Settings > Controls.
// Copyright (c) 2026 RAD Films - Roman Alurkoff. MIT License.
//
//   dotnet run --project tools/ZeroSetup -- <plugin data folder> <Chromaforge --control port>

using System.Net.Sockets;
using System.Text;
using System.Text.Json.Nodes;
using Chromaforge.Surface.Client;

if (args.Length < 2) { Console.WriteLine("usage: ZeroSetup <plugin data folder> <control port>"); return 2; }
var dir = args[0];
int ctlPort = int.Parse(args[1]);
int failures = 0;
void Check(bool ok, string what) { Console.WriteLine($"{(ok ? "PASS" : "FAIL")}  {what}"); if (!ok) failures++; }

int Port()
{
    var j = JsonNode.Parse(File.ReadAllText(Path.Combine(dir, "config.json")));
    return j?["port"]?.GetValue<int>() ?? 7981;
}
var t0 = DateTime.UtcNow;
while (!File.Exists(Path.Combine(dir, "token.txt")) && DateTime.UtcNow - t0 < TimeSpan.FromSeconds(20)) Thread.Sleep(100);
Check(File.Exists(Path.Combine(dir, "token.txt")), "Chromaforge wrote a token into the plugin's data folder");
Check(File.Exists(Path.Combine(dir, "config.json")), $"and its port ({(File.Exists(Path.Combine(dir, "config.json")) ? Port() : 0)})");

using var client = new SurfaceClient("127.0.0.1", Port, "chromaforge-logi", "Chromaforge plugin for Logi Options+", "mx-creative-console",
    new FileTokenStore(Path.Combine(dir, "token.txt")), s => Console.WriteLine($"      client: {s}"));
client.Start();
t0 = DateTime.UtcNow;
while (client.State != SurfaceState.Live && DateTime.UtcNow - t0 < TimeSpan.FromSeconds(30)) Thread.Sleep(50);
var took = (DateTime.UtcNow - t0).TotalSeconds;
Check(client.State == SurfaceState.Live, $"live without pairing ({took:0.0} s)");
Check(client.PairCode == null, "no pairing code was asked for");

using var ctl = new TcpClient("127.0.0.1", ctlPort);
var s = ctl.GetStream();
var r = new StreamReader(s, new UTF8Encoding(false));
long id = 0;
JsonNode? Ctl(string m, JsonObject? p = null)
{
    var n = ++id;
    var b = Encoding.UTF8.GetBytes(new JsonObject { ["id"] = n, ["method"] = m, ["params"] = p ?? new JsonObject() }.ToJsonString() + "\n");
    s.Write(b);
    while (true) { var l = r.ReadLine(); if (l == null) return null; var v = JsonNode.Parse(l); if (v?["id"]?.GetValue<long>() == n) return v; }
}
JsonNode? Exec(string c, JsonObject? p = null) => Ctl("engine.execute", new JsonObject { ["command"] = c, ["params"] = p ?? new JsonObject() });

var page1 = client.Layout?["keyPage"]?.ToString();
Exec("app.settings", new JsonObject { ["tab"] = "controls" });
Thread.Sleep(800);
Check(client.Layout?["context"]?.ToString()?.StartsWith("dialog") != true, $"Settings open: the keys keep their page ({client.Layout?["keyPage"]}), not a dialog page");
Ctl("ui.clickWidget", new JsonObject { ["id"] = "button:surfacePage-1" });
Thread.Sleep(800);
var page2 = client.Layout?["keyPage"]?.ToString();
Check(page2 != null && page2 != page1, $"picking another page in the editor shows it on the keys ({page1} -> {page2})");
// change key 5 in the editor: the keys follow within a second
Ctl("ui.clickWidget", new JsonObject { ["id"] = "button:surfaceControl-key.5" });
Thread.Sleep(300);
// search the action list as a user would
Ctl("ui.clickWidget", new JsonObject { ["id"] = "field:surfaceActionSearch" });
Ctl("ui.text", new JsonObject { ["text"] = "Undo" });
Thread.Sleep(600);
Ctl("ui.clickWidget", new JsonObject { ["id"] = "item:surfaceAction-c.edit.undo" });
Thread.Sleep(1000);
var key5 = client.Layout?["keys"]?.AsArray().FirstOrDefault(k => k?["control"]?.ToString() == "key.5");
Check(key5?["binding"]?["command"]?.ToString() == "edit.undo" && key5?["icon"]?.ToString() == "undo", $"an assignment made in the editor reaches the keys ({key5?["label"]}, icon {key5?["icon"]})");
Ctl("ui.dialog.cancel");
Console.WriteLine(failures == 0 ? "\nAll checks passed." : $"\n{failures} check(s) failed.");
return failures == 0 ? 0 : 1;
