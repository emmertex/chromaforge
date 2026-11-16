// Generates the default Logi Options+ profiles shipped with the Chromaforge plugin.
// Copyright (c) 2026 RAD Films - Roman Alurkoff. MIT License.
// Ported from TactileBridge's tools/GenerateProfiles.cs (the author's own): the Logi profile
// package format (.lp5: a ZIP of ProfileInfo.json, ApplicationInfo.json and metadata).
//
// The keys and dials only forward to Chromaforge, which decides what they do, so one profile per
// device covers every Chromaforge profile (Default, Wedding, Portrait, …): nothing to assign by hand.
//   DefaultProfile70.lp5  MX Creative Keypad: pages 1-3, keys p{page}k{key}
//   DefaultProfile71.lp5  MX Creative Dialpad: buttons pad1-4, the dial and the roller
//   DefaultProfile72.lp5  Actions Ring (Logi Options+' on-screen ring): slots ring1-8
//   DefaultProfile15.lp5  Loupedeck+: every knob and button by Loupedeck's own number
// (the DefaultProfile<device>.lp5 names are what the Logi Plugin Service installs by itself when
// it installs the plugin package).

using System.IO.Compression;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

const string Plugin = "Chromaforge";
const string App = "@_chromaforge";

var outDir = args.Length > 0 ? args[0] : Path.Combine("src", "ChromaforgePlugin", "profiles");
Directory.CreateDirectory(outDir);

string Key(string param) => $"${Plugin}___ChromaforgeKey___{param}";
string Dial(string param) => $"${Plugin}___ChromaforgeDial___{param}";

// stable ids: reinstalling never adds a second copy
string Id(string seed) => Convert.ToHexString(MD5.HashData(Encoding.UTF8.GetBytes("chromaforge-logi/" + seed)));

Dictionary<string, object?> D(params (string k, object? v)[] p) => p.ToDictionary(x => x.k, x => x.v);

Dictionary<string, object?> Control(int id, string? press = null, string? rotate = null) =>
    D(("$type", "Loupedeck.Service.Devices.Loupedeck7Devices.ProfileLayoutControl7, LoupedeckService"),
      ("controlId", id), ("pressAction", press), ("rotateAction", rotate));

Dictionary<string, object?> Page(string seed, string name, List<Dictionary<string, object?>> controls) =>
    D(("$type", "Loupedeck.Service.Devices.Loupedeck7Devices.ProfileLayoutPage7, LoupedeckService"),
      ("name", Id(seed)), ("displayName", name), ("description", null), ("controls", controls));

// the layout of the MX Creative Console's devices: pages of controls
Dictionary<string, object?> Layout7(string device, List<Dictionary<string, object?>> press, List<Dictionary<string, object?>> rotate)
{
    var workspace = Id("workspace/" + device);
    return D(
        ("$type", "Loupedeck.Service.Devices.Loupedeck7Devices.ProfileLayout7, LoupedeckService"),
        ("deviceType", device), ("profileFlags", "None"),
        ("layoutModes", new object[] {
            D(("$type", "Loupedeck.Service.Devices.Loupedeck7Devices.ProfileLayoutMode7, LoupedeckService"),
              ("deviceType", device), ("modeName", "Main"), ("parentModeName", null), ("actions", null),
              ("dynamicButtonPages", null), ("dynamicEncoderPages", null),
              ("workspaces", new object[] {
                  D(("$type", "Loupedeck.Service.Devices.Loupedeck7Devices.ProfileLayoutWorkspace7, LoupedeckService"),
                    ("name", workspace), ("displayName", "Workspace 1"), ("description", null),
                    ("pressPages", press), ("rotatePages", rotate))
              }),
              ("homeWorkspaceName", workspace))
        }));
}

// the Loupedeck+'s layout: one list of presses and one of turns, by the controls' numbers
Dictionary<string, object?> Layout10(string device, string?[] press, string?[] rotate)
{
    var workspace = Id("workspace/" + device);
    return D(
        ("$type", "Loupedeck.Service.Devices.Loupedeck1Devices.ProfileLayout10, LoupedeckService"),
        ("deviceType", device), ("profileFlags", "None"),
        ("layoutModes", new object[] {
            D(("$type", "Loupedeck.Service.Devices.Loupedeck1Devices.ProfileLayoutMode10, LoupedeckService"),
              ("deviceType", device), ("modeName", "Main"), ("parentModeName", null), ("actions", null),
              ("dynamicButtonPages", null), ("dynamicEncoderPages", null),
              ("workspaces", new object[] {
                  D(("$type", "Loupedeck.Service.Devices.Loupedeck1Devices.ProfileLayoutWorkspace10, LoupedeckService"),
                    ("name", workspace), ("displayName", null), ("description", null),
                    ("pressActions", press), ("rotateActions", rotate))
              }))
        }));
}

void Write(string file, string device, Dictionary<string, object?> layout)
{
    var profile = Id("profile/" + device);
    var opts = new JsonSerializerOptions { WriteIndented = true };
    var appInfo = D(("$type", "Loupedeck.Service.SupportedApplicationInfo, LoupedeckService"),
        ("name", App), ("displayName", "Chromaforge"), ("description", null), ("deviceType", device),
        ("nativePluginName", Plugin), ("hasNativePlugin", true), ("processOrBundleName", "chromaforge"),
        ("modes", new object[] { D(("$type", "Loupedeck.Service.ApplicationMode, LoupedeckService"), ("name", "Main"), ("parentModeName", null), ("displayName", "Main")) }),
        ("defaultProfileName", profile), ("isEnabled", true));
    var info = D(
        ("$type", "Loupedeck.Service.ApplicationProfile, LoupedeckService"),
        ("name", profile), ("profileFlags", "None"), ("displayName", "Chromaforge"),
        ("description", "Every key and dial is set in Chromaforge: Settings, Controls"),
        ("deviceType", device), ("applicationName", App), ("nativePluginName", Plugin), ("hasNativePlugin", true),
        ("additionalNativePluginNames", Array.Empty<string>()),
        ("lastModifiedTimeUtc", "2026-10-14T00:00:00.0000000Z"),
        ("profileSettings", D(("$type", "Loupedeck.DictionaryNoCase`1[[System.String, System.Private.CoreLib]], PluginApi"))),
        ("actionImages90", null), ("actionImages60", null), ("wheelImages", null), ("actionColors", null),
        ("layout", layout),
        ("macroCommands", Array.Empty<object>()), ("macroAdjustments", Array.Empty<object>()),
        ("profileCommands", Array.Empty<object>()), ("profileAdjustments", Array.Empty<object>()),
        ("conversionHistory", ""), ("profileActions", Array.Empty<object>()));
    var yaml = $"type: Profile5\nname: {profile}\ndisplayName: Chromaforge\nversion: 0.1.0\ndescription: ''\nauthor: RAD Films - Roman Alurkoff\n";
    var path = Path.Combine(outDir, file);
    if (File.Exists(path)) File.Delete(path);
    using var zip = ZipFile.Open(path, ZipArchiveMode.Create);
    void Add(string name, byte[] data)
    {
        var e = zip.CreateEntry(name, CompressionLevel.Optimal);
        e.LastWriteTime = new DateTimeOffset(2026, 10, 14, 0, 0, 0, TimeSpan.Zero);
        using var s = e.Open();
        s.Write(data);
    }
    Add("ApplicationInfo.json", Encoding.UTF8.GetBytes(JsonSerializer.Serialize(appInfo, opts)));
    Add("ProfileInfo.json", Encoding.UTF8.GetBytes(JsonSerializer.Serialize(info, opts)));
    Add("metadata/LoupedeckPackage.yaml", Encoding.UTF8.GetBytes(yaml));
    Add("metadata/AdvancedInfo.json", Encoding.UTF8.GetBytes(JsonSerializer.Serialize(D(("additionalPluginNames", new[] { Plugin })), opts)));
    var icon = Path.Combine(Path.GetDirectoryName(Path.GetFullPath(outDir))!, "metadata", "Icon256x256.png");
    if (File.Exists(icon)) Add("ApplicationIcon.png", File.ReadAllBytes(icon));
    foreach (var dir in new[] { "ActionIcons/", "ActionImages/", "ActionMetadata/" }) zip.CreateEntry(dir);
    Console.WriteLine($"  {file} ({device}, profile {profile})");
}

// MX Creative Keypad: three pages of nine keys
var keypad = new List<Dictionary<string, object?>>();
for (int page = 1; page <= 3; page++)
    keypad.Add(Page($"keypad/{page}", $"Page {page}", Enumerable.Range(1, 9).Select(k => Control(k - 1, press: Key($"p{page}k{k}"))).ToList()));
Write("DefaultProfile70.lp5", "Loupedeck70", Layout7("Loupedeck70", keypad, new()));

// MX Creative Dialpad: four buttons, the dial and the roller
var pads = Page("dialpad/buttons", "Buttons", Enumerable.Range(1, 4).Select(i => Control(i - 1, press: Key($"pad{i}"))).ToList());
var dials = Page("dialpad/dials", "Dials", new() { Control(0, rotate: Dial("dial")), Control(1, rotate: Dial("roller")) });
Write("DefaultProfile71.lp5", "Loupedeck71", Layout7("Loupedeck71", new() { pads }, new() { dials }));

// Actions Ring: eight slots, the same on every Chromaforge page (set in Chromaforge)
var ring = Page("ring/slots", "Page (1)", Enumerable.Range(1, 8).Select(i => Control(i - 1, press: Key($"ring{i}"))).ToList());
Write("DefaultProfile72.lp5", "Loupedeck72", Layout7("Loupedeck72", new() { ring }, new()));

// Loupedeck+: each knob and button forwards its number; Chromaforge knows what it does. Picked
// by itself when Chromaforge is the app in front (the application's process name).
var ldPress = new string?[127];
var ldRotate = new string?[127];
foreach (var n in Enumerable.Range(1, 48).Where(n => n != 43)) ldRotate[n] = $"${Plugin}___LoupedeckPlusKnob___r{n}";
foreach (var n in Enumerable.Range(33, 16).Where(n => n != 43).Concat(Enumerable.Range(49, 6)).Concat(Enumerable.Range(66, 39)).Append(110).Concat(Enumerable.Range(114, 3)))
    ldPress[n] = $"${Plugin}___LoupedeckPlusButton___b{n}";
Write("DefaultProfile15.lp5", "Loupedeck15", Layout10("Loupedeck15", ldPress, ldRotate));
Console.WriteLine("Done.");
