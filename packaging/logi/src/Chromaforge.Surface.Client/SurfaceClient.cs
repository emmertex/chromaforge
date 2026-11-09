// Chromaforge surface-protocol client (docs/control-surfaces.md in the Chromaforge repository).
// Copyright (c) 2026 RAD Films - Roman Alurkoff. MIT License.

using System.Net.Sockets;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace Chromaforge.Surface.Client;

/// <summary>Where the connection to Chromaforge stands.</summary>
public enum SurfaceState
{
    /// Chromaforge isn't running, or isn't listening for surfaces.
    Disconnected,
    /// Connected, waiting for the user to approve pairing in Chromaforge (Settings, Controls).
    Pairing,
    /// Chromaforge refused: control surfaces are switched off, or pairing was denied.
    Refused,
    /// Paired and subscribed: layouts arrive, input goes out.
    Live,
}

/// <summary>
/// A JSON-lines connection to Chromaforge's control-surface port: pairs (once), subscribes to a
/// device's layout, sends dial ticks and key presses, and raises <see cref="LayoutChanged"/> when
/// Chromaforge pushes a new layout. Reconnects by itself. Never throws to the caller.
/// </summary>
public sealed class SurfaceClient : IDisposable
{
    public const int Protocol = 1;

    private readonly string _host;
    private readonly Func<int> _port;
    private readonly string _clientId;
    private readonly string _clientName;
    private readonly string _device;
    private readonly ITokenStore _tokens;
    private readonly Action<string>? _log;

    private readonly object _lock = new();
    private TcpClient? _tcp;
    private NetworkStream? _stream;
    private long _nextId;
    private readonly Dictionary<long, TaskCompletionSource<JsonNode?>> _waiting = new();
    private readonly CancellationTokenSource _stop = new();
    private Thread? _thread;

    private JsonNode? _layout;
    private JsonNode? _lastLayout;
    private int _lastPort;
    private JsonNode? _catalog;
    private readonly string? _cacheDir;
    private string[] _watch = Array.Empty<string>();
    private SurfaceState _state = SurfaceState.Disconnected;
    private string? _pairCode;
    private string _status = "Waiting for Chromaforge";
    private readonly Dictionary<string, string> _icons = new(StringComparer.Ordinal);

    /// <summary>A new layout arrived (or the connection state changed).</summary>
    public event Action? LayoutChanged;

    /// <summary>Chromaforge sent a different action catalogue (see <see cref="Catalog"/>).</summary>
    public event Action? CatalogChanged;

    /// <param name="cacheDir">Where the last layout, the icons and the action catalogue are kept
    /// between runs, so keys and the plugin's action list show Chromaforge's icons and names even
    /// before Chromaforge is started (null: nothing is kept).</param>
    public SurfaceClient(string host, int port, string clientId, string clientName, string device, ITokenStore tokens, Action<string>? log = null, string? cacheDir = null)
        : this(host, () => port, clientId, clientName, device, tokens, log, cacheDir) { }

    /// <param name="port">Asked before every connection attempt, so a changed setting (the plugin's
    /// <c>config.json</c>) takes effect without restarting anything.</param>
    public SurfaceClient(string host, Func<int> port, string clientId, string clientName, string device, ITokenStore tokens, Action<string>? log = null, string? cacheDir = null)
    {
        _host = host;
        _port = port;
        _clientId = clientId;
        _clientName = clientName;
        _device = device;
        _tokens = tokens;
        _log = log;
        _cacheDir = cacheDir;
        LoadCache();
    }

    public SurfaceState State { get { lock (_lock) return _state; } }

    /// <summary>The pairing code to show while <see cref="State"/> is <see cref="SurfaceState.Pairing"/>.</summary>
    public string? PairCode { get { lock (_lock) return _pairCode; } }

    /// <summary>A short status line for keys when not live.</summary>
    public string Status { get { lock (_lock) return _status; } }

    /// <summary>The last layout Chromaforge pushed (null until live).</summary>
    /// Read-only: the same snapshot is handed to every caller (a key redraw must not copy the whole
    /// layout: 40 keys each cloning it on every push stalled the Logi service). Never modify it.
    public JsonNode? Layout { get { lock (_lock) return _layout; } }

    /// <summary>The live layout, else the last everyday one seen (also from an earlier run), to
    /// draw the keys dimmed while Chromaforge is not there.</summary>
    public JsonNode? LastLayout { get { lock (_lock) return _layout ?? _lastLayout; } }

    /// <summary>The last everyday layout (never a dialog's or a tool's page), live or kept.</summary>
    /// Only one seen on the port the plugin is set to now: a test copy's layout on another port is
    /// never shown for the everyday app.
    public JsonNode? BaseLayout
    {
        get
        {
            int port;
            try { port = _port(); } catch (Exception) { port = 7981; }
            lock (_lock)
                return _lastLayout?["_port"]?.GetValue<int>() == port ? _lastLayout : null;
        }
    }

    private long _savedAt;

    /// <summary>A layout worth keeping for when Chromaforge is closed: the profile's everyday pages,
    /// not a dialog's OK / Cancel or a tool's page that happened to be up.</summary>
    public static bool IsEverydayLayout(JsonNode? layout)
    {
        var ctx = layout?["context"]?.ToString() ?? "";
        return ctx is "" or "default" or "library" or "develop" || ctx.StartsWith("develop.", StringComparison.Ordinal);
    }

    /// <summary>A layout as the read-only snapshot callers share: a copy whose every object and
    /// array is built now (a parsed JsonNode builds them lazily, on first read, which is not safe
    /// from several threads at once).</summary>
    private static JsonNode? Snapshot(JsonNode? layout)
    {
        var copy = layout?.DeepClone();
        Materialize(copy, 0);
        return copy;
    }

    private static void Materialize(JsonNode? n, int depth)
    {
        if (depth > 64) return;
        switch (n)
        {
            case JsonObject o:
                foreach (var (_, v) in o) Materialize(v, depth + 1);
                break;
            case JsonArray a:
                foreach (var v in a) Materialize(v, depth + 1);
                break;
        }
    }

    private void Remember(JsonNode? layout)
    {
        // called under _lock
        if (layout == null || !IsEverydayLayout(layout)) return;
        var kept = layout.DeepClone();
        kept["_port"] = _lastPort;
        Materialize(kept, 0);
        _lastLayout = kept;
        // kept on disk now and then while live, so a closed or crashed service still has it
        if (Environment.TickCount64 - _savedAt > 5000)
        {
            _savedAt = Environment.TickCount64;
            SaveCache("layout.json", _lastLayout.ToJsonString());
        }
    }

    /// <summary>Chromaforge's action catalogue (<c>surface.catalog</c>: <c>groups</c>, <c>entries</c>
    /// with <c>key</c>, <c>group</c>, <c>label</c>, <c>icon</c>, <c>value</c>), live or from the
    /// last run; null if Chromaforge was never reached.</summary>
    public JsonNode? Catalog { get { lock (_lock) return _catalog?.DeepClone(); } }

    /// <summary>Goes up whenever icons arrive (keys whose icon came late redraw).</summary>
    public long IconsVersion => Interlocked.Read(ref _iconsVersion);
    private long _iconsVersion;

    /// <summary>An icon's SVG, if Chromaforge has sent it.</summary>
    public string? Icon(string name)
    {
        lock (_lock) return _icons.TryGetValue(name, out var s) ? s : null;
    }

    /// <summary>The port of the current connection (0: none).</summary>
    public int ConnectedPort { get { lock (_lock) return _tcp != null ? _lastPort : 0; } }

    /// <summary>The port the client would connect to now (its setting).</summary>
    public int WantedPort
    {
        get
        {
            try { return _port(); } catch (Exception) { return 7981; }
        }
    }

    /// <summary>Drop the connection and connect again (the port setting changed).</summary>
    public void Reconnect() => Close();

    /// <summary>Start the connection loop (on its own thread).</summary>
    public void Start()
    {
        if (_thread != null) return;
        _thread = new Thread(Run) { IsBackground = true, Name = "Chromaforge surface client" };
        _thread.Start();
        _sender = new Thread(SendLoop) { IsBackground = true, Name = "Chromaforge surface input" };
        _sender.Start();
    }

    // ------------------------------------------------------------------------------- input

    private readonly InputQueue _input = new();
    private readonly AutoResetEvent _inputReady = new(false);
    private Thread? _sender;

    /// <summary>How long the sender waits for Chromaforge's reply before it sends the next batch
    /// anyway (a lost reply never stalls the console for long).</summary>
    public static readonly TimeSpan InFlightLimit = TimeSpan.FromSeconds(2);

    /// <summary>Input waiting to be sent (tests and diagnostics).</summary>
    public int QueuedInput => _input.Count;

    /// <summary>Requests <c>surface.input</c> sent so far (tests and diagnostics).</summary>
    public long InputRequests => Interlocked.Read(ref _inputRequests);
    private long _inputRequests;

    /// <summary>An encoder turned (positive = clockwise).</summary>
    public void Turn(string control, int ticks) => Queue(() => _input.Turn("control", control, ticks));

    /// <summary>A key or button went down (<paramref name="page"/>: the key page the device shows, 1-based).</summary>
    public void Press(string control, int? page = null) => Queue(() => _input.Button("control", control, true, page));

    public void Release(string control) => Queue(() => _input.Button("control", control, false));

    /// <summary>An action from the plugin's own list turned (a value: moves it).</summary>
    public void TurnAction(string key, int ticks) => Queue(() => _input.Turn("action", key, ticks));

    /// <summary>An action from the plugin's own list pressed or released (a value: a press resets it).</summary>
    public void PressAction(string key, bool down) => Queue(() => _input.Button("action", key, down));

    /// <summary>The values whose live readout the plugin wants in the layout (<c>values</c>).</summary>
    public void Watch(IEnumerable<string> targets)
    {
        var t = targets.Distinct(StringComparer.Ordinal).OrderBy(x => x, StringComparer.Ordinal).Take(128).ToArray();
        lock (_lock)
        {
            if (t.SequenceEqual(_watch)) return;
            _watch = t;
        }
        SendWatch();
    }

    private void SendWatch()
    {
        string[] t;
        lock (_lock) t = _watch;
        if (State != SurfaceState.Live || t.Length == 0) return;
        var arr = new JsonArray();
        foreach (var x in t) arr.Add(x);
        _ = Request("surface.watch", new JsonObject { ["targets"] = arr }, TimeSpan.FromSeconds(10));
    }

    /// <summary>Input never waits on the device's thread: it is queued (merged, see
    /// <see cref="InputQueue"/>) and the sender thread takes it from there.</summary>
    private void Queue(Action add)
    {
        if (State != SurfaceState.Live) return;
        try
        {
            add();
            _inputReady.Set();
        }
        catch (Exception e) { Log($"input: {e.Message}"); }
    }

    /// <summary>Sends what is queued as one request, then waits for its reply (at most
    /// <see cref="InFlightLimit"/>) before the next: one request in flight, so input merges here
    /// while Chromaforge is busy instead of queuing up in the socket. Never throws.</summary>
    private void SendLoop()
    {
        while (!Stopping)
        {
            try
            {
                _inputReady.WaitOne(TimeSpan.FromMilliseconds(200));
                while (!Stopping)
                {
                    if (State != SurfaceState.Live) { _input.Clear(); break; }
                    var batch = _input.Take();
                    if (batch == null) break;
                    Interlocked.Increment(ref _inputRequests);
                    var reply = Request("surface.input", new JsonObject { ["events"] = batch }, InFlightLimit);
                    try { reply.Wait(InFlightLimit + TimeSpan.FromMilliseconds(50)); }
                    catch (AggregateException) { /* the request never throws; nothing to do */ }
                }
            }
            catch (Exception e)
            {
                // a thread's exception would end the Logi service
                Log($"sender: {e.Message}");
            }
        }
    }

    // ------------------------------------------------------------------------------- loop

    private void Log(string s)
    {
        try { _log?.Invoke(s); } catch { /* logging never breaks the client */ }
    }

    private void SetState(SurfaceState s, string status, string? code = null)
    {
        bool changed;
        lock (_lock)
        {
            changed = _state != s || _status != status || _pairCode != code;
            _state = s;
            _status = status;
            _pairCode = code;
            if (s != SurfaceState.Live && _layout != null)
            {
                _layout = null;
                if (_lastLayout != null) SaveCache("layout.json", _lastLayout.ToJsonString());
            }
        }
        if (changed)
        {
            Log($"state {s}: {status}");
            Raise();
        }
    }

    private void Raise()
    {
        try { LayoutChanged?.Invoke(); } catch (Exception e) { Log($"LayoutChanged handler: {e.Message}"); }
    }

    /// <summary>Wait, or less when stopping. Never throws.</summary>
    private void Pause(TimeSpan t)
    {
        try { _stop.Token.WaitHandle.WaitOne(t); }
        catch (Exception) { /* stopping */ }
    }

    private bool Stopping
    {
        get
        {
            try { return _stop.IsCancellationRequested; }
            catch (Exception) { return true; }
        }
    }

    // A thread's exception that escapes kills the whole Logi Plugin Service (every plugin, and
    // Options+ waits for it): nothing may leave this method.
    private void Run()
    {
        while (!Stopping)
        {
            try
            {
                try
                {
                    Session();
                }
                catch (Exception e) when (e is IOException or SocketException or ObjectDisposedException or JsonException or InvalidOperationException or TimeoutException)
                {
                    Log($"connection: {e.Message}");
                }
                Close();
                if (Stopping) break;
                if (State != SurfaceState.Refused)
                    SetState(SurfaceState.Disconnected, "Start Chromaforge");
            }
            catch (Exception e)
            {
                Log($"unexpected: {e}");
            }
            Pause(TimeSpan.FromSeconds(2));
        }
    }

    private void Close()
    {
        lock (_lock)
        {
            try { _stream?.Dispose(); } catch { }
            try { _tcp?.Dispose(); } catch { }
            _stream = null;
            _tcp = null;
            foreach (var w in _waiting.Values) w.TrySetResult(null);
            _waiting.Clear();
        }
        // input from before the drop must never replay on the next connection
        _input.Clear();
    }

    /// <summary>One connection: hello, pair if needed, subscribe, then read until it drops.</summary>
    private void Session()
    {
        var tcp = new TcpClient { NoDelay = true };
        int port;
        try { port = _port(); } catch (Exception) { port = 7981; }
        if (port is <= 0 or > 65535) port = 7981;
        if (port != _lastPort) { Log($"connecting to {_host}:{port}"); _lastPort = port; }
        tcp.Connect(_host, port);
        var stream = tcp.GetStream();
        lock (_lock)
        {
            _tcp = tcp;
            _stream = stream;
        }
        var reader = new Thread(() => ReadLoop(stream)) { IsBackground = true, Name = "Chromaforge surface reader" };
        reader.Start();

        var token = _tokens.Load();
        var hello = Call("surface.hello", new JsonObject { ["protocol"] = Protocol, ["client"] = _clientId, ["name"] = _clientName, ["token"] = token });
        if (hello?["ok"]?.GetValue<bool>() != true)
        {
            var error = hello?["error"]?.ToString() ?? "Chromaforge refused";
            // Chromaforge is starting or its window is hidden: not a refusal, try again shortly
            if (error.Contains("drew no frame", StringComparison.Ordinal)) { Log($"busy: {error}"); return; }
            SetState(SurfaceState.Refused, error);
            return;
        }
        if (hello["result"]?["paired"]?.GetValue<bool>() != true)
        {
            if (!PairAndReconnect()) return;
            hello = Call("surface.hello", new JsonObject { ["protocol"] = Protocol, ["client"] = _clientId, ["token"] = _tokens.Load() });
            Log($"paired: {hello?["result"]?["paired"]}");
            if (hello?["result"]?["paired"]?.GetValue<bool>() != true)
            {
                SetState(SurfaceState.Refused, "Pairing failed");
                return;
            }
        }
        LoadCatalog();
        LoadIcons();
        var sub = Call("surface.subscribe", new JsonObject { ["device"] = _device });
        if (sub?["ok"]?.GetValue<bool>() != true)
        {
            var err = sub?["error"]?.ToString() ?? "subscribe failed";
            if (err.Contains("isn't paired")) _tokens.Save(null);
            SetState(SurfaceState.Refused, err);
            return;
        }
        lock (_lock)
        {
            _layout = Snapshot(sub["result"]?["layout"]);
            Remember(_layout);
        }
        SetState(SurfaceState.Live, "Chromaforge");
        SendWatch();
        Raise();
        reader.Join();
    }

    /// <summary>Ask to pair and wait (polling) until the user answers in Chromaforge.</summary>
    private bool PairAndReconnect()
    {
        var ask = Call("surface.pair", new JsonObject { ["client"] = _clientId, ["name"] = _clientName });
        if (ask?["ok"]?.GetValue<bool>() != true)
        {
            SetState(SurfaceState.Refused, ask?["error"]?.ToString() ?? "pairing refused");
            return false;
        }
        var request = ask["result"]?["request"]?.ToString();
        var code = ask["result"]?["code"]?.ToString();
        if (request == null) return false;
        SetState(SurfaceState.Pairing, "Approve in Chromaforge", code);
        var tokenBefore = _tokens.Load();
        while (!Stopping)
        {
            Pause(TimeSpan.FromSeconds(1));
            // Chromaforge trusted this plugin meanwhile (it wrote a token into the plugin's data
            // folder): connect with it instead of waiting for the code to be approved
            var token = _tokens.Load();
            if (!string.IsNullOrEmpty(token) && token != tokenBefore)
            {
                Log("trusted by Chromaforge");
                return true;
            }
            var poll = Call("surface.pair", new JsonObject { ["request"] = request });
            Log($"pairing: {poll?["result"]?["status"] ?? poll?["error"]}");
            if (poll == null) return false;
            if (poll["ok"]?.GetValue<bool>() != true)
            {
                // expired: ask again
                return PairAndReconnect();
            }
            var status = poll["result"]?["status"]?.ToString();
            if (status == "approved")
            {
                _tokens.Save(poll["result"]?["token"]?.ToString());
                return true;
            }
            if (status == "denied")
            {
                SetState(SurfaceState.Refused, "Pairing denied in Chromaforge");
                return false;
            }
        }
        return false;
    }

    private void LoadIcons()
    {
        var r = Call("surface.icons", new JsonObject());
        if (r?["result"] is JsonObject icons)
        {
            lock (_lock)
            {
                foreach (var (name, svg) in icons)
                    if (svg != null) _icons[name] = svg.ToString();
            }
            Interlocked.Increment(ref _iconsVersion);
            SaveCache("icons.json", icons.ToJsonString());
        }
    }

    private void LoadCatalog()
    {
        var r = Call("surface.catalog", new JsonObject());
        if (r?["result"] is not JsonObject c || c["entries"] is not JsonArray) return;
        var text = c.ToJsonString();
        bool changed;
        lock (_lock)
        {
            changed = _catalog?.ToJsonString() != text;
            _catalog = c.DeepClone();
        }
        if (!changed) return;
        SaveCache("catalog.json", text);
        try { CatalogChanged?.Invoke(); } catch (Exception e) { Log($"CatalogChanged handler: {e.Message}"); }
    }

    // ------------------------------------------------------------------------------- cache

    private void LoadCache()
    {
        if (_cacheDir == null) return;
        try
        {
            var icons = ReadCache("icons.json");
            if (icons is JsonObject o)
                foreach (var (name, svg) in o)
                    if (svg != null) _icons[name] = svg.ToString();
            _catalog = ReadCache("catalog.json") is JsonObject c && c["entries"] is JsonArray ? c : null;
            _lastLayout = ReadCache("layout.json") is JsonObject l && IsEverydayLayout(l) ? Snapshot(l) : null;
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or JsonException or InvalidOperationException)
        {
            Log($"cache: {e.Message}");
        }
    }

    private JsonNode? ReadCache(string name)
    {
        var path = Path.Combine(_cacheDir!, name);
        if (!File.Exists(path) || new FileInfo(path).Length > 16 << 20) return null;
        return JsonNode.Parse(File.ReadAllText(path));
    }

    private void SaveCache(string name, string text)
    {
        if (_cacheDir == null) return;
        try
        {
            Directory.CreateDirectory(_cacheDir);
            var path = Path.Combine(_cacheDir, name);
            var tmp = path + ".tmp";
            File.WriteAllText(tmp, text);
            File.Move(tmp, path, true);
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            Log($"cache {name}: {e.Message}");
        }
    }

    private JsonNode? Call(string method, JsonObject p) => Request(method, p, TimeSpan.FromSeconds(30)).GetAwaiter().GetResult();

    /// <summary>Send a request; the reply (or null if the connection drops or times out).</summary>
    public async Task<JsonNode?> Request(string method, JsonObject p, TimeSpan timeout)
    {
        NetworkStream? stream;
        var tcs = new TaskCompletionSource<JsonNode?>(TaskCreationOptions.RunContinuationsAsynchronously);
        long id;
        lock (_lock)
        {
            stream = _stream;
            if (stream == null) return null;
            id = ++_nextId;
            _waiting[id] = tcs;
        }
        var line = new JsonObject { ["id"] = id, ["method"] = method, ["params"] = p }.ToJsonString() + "\n";
        var bytes = Encoding.UTF8.GetBytes(line);
        try
        {
            lock (stream) stream.Write(bytes, 0, bytes.Length);
        }
        catch (Exception e) when (e is IOException or ObjectDisposedException or InvalidOperationException)
        {
            lock (_lock) _waiting.Remove(id);
            return null;
        }
        var done = await Task.WhenAny(tcs.Task, Task.Delay(timeout)).ConfigureAwait(false);
        lock (_lock) _waiting.Remove(id);
        return done == tcs.Task ? tcs.Task.Result : null;
    }

    private void ReadLoop(NetworkStream stream)
    {
        try
        {
            using var r = new StreamReader(stream, new UTF8Encoding(false), false, 1 << 16, leaveOpen: true);
            while (!Stopping)
            {
                var line = r.ReadLine();
                if (line == null) break;
                if (line.Length == 0) continue;
                JsonNode? msg;
                try { msg = JsonNode.Parse(line); }
                catch (JsonException) { continue; }
                if (msg == null) continue;
                if (msg["event"] is JsonNode ev)
                {
                    OnEvent(ev.ToString(), msg);
                    continue;
                }
                if (msg["id"]?.GetValueKind() == JsonValueKind.Number)
                {
                    var id = msg["id"]!.GetValue<long>();
                    TaskCompletionSource<JsonNode?>? w;
                    lock (_lock) _waiting.TryGetValue(id, out w);
                    w?.TrySetResult(msg);
                }
            }
        }
        catch (Exception e)
        {
            // any exception here would end the Logi service itself
            Log($"read: {e.Message}");
        }
        finally
        {
            Close();
        }
    }

    private void OnEvent(string name, JsonNode msg)
    {
        switch (name)
        {
            case "surface.layout":
                lock (_lock)
                {
                    _layout = Snapshot(msg["layout"]);
                    Remember(_layout);
                }
                Raise();
                break;
            case "surface.unpaired":
                _tokens.Save(null);
                Close();
                break;
        }
    }

    public void Dispose()
    {
        lock (_lock)
            if (_lastLayout != null) SaveCache("layout.json", _lastLayout.ToJsonString());
        try { _stop.Cancel(); } catch (Exception) { }
        Close();
        // the token source is not disposed: the connection thread may still be looking at it,
        // and an ObjectDisposedException there would take the Logi service down
        try { _thread?.Join(TimeSpan.FromSeconds(3)); } catch (Exception) { }
        try { _inputReady.Set(); _sender?.Join(TimeSpan.FromSeconds(1)); } catch (Exception) { }
    }
}

/// <summary>Keeps the pairing token between runs.</summary>
public interface ITokenStore
{
    string? Load();
    void Save(string? token);
}

/// <summary>A token in a file (the plugin's data folder).</summary>
public sealed class FileTokenStore : ITokenStore
{
    private readonly string _path;
    public FileTokenStore(string path) { _path = path; }

    public string? Load()
    {
        try { return File.Exists(_path) ? File.ReadAllText(_path).Trim() : null; }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException) { return null; }
    }

    public void Save(string? token)
    {
        try
        {
            if (token == null) { if (File.Exists(_path)) File.Delete(_path); return; }
            Directory.CreateDirectory(Path.GetDirectoryName(_path)!);
            File.WriteAllText(_path, token);
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException) { }
    }
}
