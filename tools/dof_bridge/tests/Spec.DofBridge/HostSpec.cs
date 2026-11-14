// Subphase 3.5 Independent Tester: the UDP host and the bridge application entry point.
using System.Diagnostics;
using System.Globalization;
using System.Net;
using System.Net.Sockets;
using Kilohertz.DofBridge;
using Xunit;

namespace Spec.DofBridge;

/// <summary>Socket tests run one at a time (shared loopback ports, timing).</summary>
internal static class Net
{
    /// <summary>Bounded liveness wait (not a timing assertion).</summary>
    public static void WaitUntil(string what, Func<bool> condition, int seconds = 20)
    {
        var deadline = Stopwatch.StartNew();
        while (!condition())
        {
            if (deadline.Elapsed > TimeSpan.FromSeconds(seconds)) Assert.Fail($"timed out waiting for {what}");
            Thread.Sleep(2);
        }
    }

    public static int FreeUdpPort()
    {
        using var probe = new Socket(AddressFamily.InterNetwork, SocketType.Dgram, ProtocolType.Udp);
        probe.Bind(new IPEndPoint(IPAddress.Loopback, 0));
        return ((IPEndPoint)probe.LocalEndPoint!).Port;
    }

    public static bool IsBound(int port)
    {
        try
        {
            using var probe = new Socket(AddressFamily.InterNetwork, SocketType.Dgram, ProtocolType.Udp);
            probe.ExclusiveAddressUse = true;
            probe.Bind(new IPEndPoint(IPAddress.Loopback, port));
            return false;
        }
        catch (SocketException)
        {
            return true;
        }
    }

    public static UdpClient Sender(int port)
    {
        var client = new UdpClient(AddressFamily.InterNetwork);
        client.Connect(IPAddress.Loopback, port);
        return client;
    }
}

[Collection("sockets")]
public class UdpHostSpec
{
    /// <summary>Replays the recording through a fresh core + UDP host, one datagram at a time.</summary>
    private static BridgeStats ReplayOverUdp(ResolvedDofMap map, List<byte[]> recording, IDofSink sink, Func<int> sinkCount, int expectedCount)
    {
        var core = new BridgeCore(map, sink);
        using var host = new UdpBridgeHost(core, new IPEndPoint(IPAddress.Loopback, 0));
        host.Start();
        int port = host.LocalEndPoint.Port;
        Assert.NotEqual(0, port);
        Assert.Equal(IPAddress.Loopback, host.LocalEndPoint.Address);

        using (var tx = Net.Sender(port))
        {
            long sent = 0;
            foreach (var d in recording)
            {
                tx.Send(d, d.Length);
                sent++;
                // One datagram at a time, as the sim sends them (a tick apart at least); loopback keeps order.
                Net.WaitUntil($"datagram {sent} to be received", () => host.Received >= sent);
            }
        }
        Net.WaitUntil("all updates to reach the sink", () => core.Stats.Accepted >= recording.Count && sinkCount() >= expectedCount);
        host.Stop();
        Assert.Equal(recording.Count, host.Received);
        return core.Stats;
    }

    [Fact]
    public void Recorded_session_over_udp_reaches_the_sink_in_order()
    {
        var map = Fixtures.Mapping("session.mapping.txt");
        var recording = Fixtures.Recording("session.bin");
        var expected = Fixtures.Expected("session.expected.txt");

        var sink = new RecordingSink();
        var stats = ReplayOverUdp(map, recording, sink, () => sink.Count, expected.Count);
        Assert.Equal(expected, sink.Updates);
        Assert.Equal(expected.Count, stats.Forwarded);
        Assert.True(stats.MaxForwardLatencyTicks > 0, "the host must stamp datagrams on its monotonic clock");
    }

    [Fact]
    public void Recorded_session_over_udp_is_forwarded_within_2_ms()
    {
        var map = Fixtures.Mapping("session.mapping.txt");
        var recording = Fixtures.Recording("session.bin");
        var expected = Fixtures.Expected("session.expected.txt");

        // Warm the forwarding path (JIT) so the measured runs time the bridge, not the compiler.
        for (int i = 0; i < 20; i++)
        {
            var warm = new BridgeCore(map, new FixedSink(expected.Count));
            foreach (var d in recording) warm.Process(d, Stopwatch.GetTimestamp());
        }

        // The bound is on the bridge (receive stamp to sink call, its own monotonic stamps, p100 over the
        // fixture). A pre-empted receive thread inflates one replay's p100 without the bridge being slow, so
        // the fixture is replayed several times and the best replay must meet the bound: a bridge that is
        // really slower than 2 ms on some update is slower in every replay.
        const int Replays = 5;
        var best = TimeSpan.MaxValue;
        var seen = new List<string>();
        for (int r = 0; r < Replays; r++)
        {
            var sink = new FixedSink(expected.Count);
            var stats = ReplayOverUdp(map, recording, sink, () => sink.Count, expected.Count);
            Assert.Equal(expected.Count, sink.Count);
            Assert.Equal(expected.Count, stats.Forwarded);
            Assert.True(stats.MaxForwardLatencyTicks > 0, "the host must stamp datagrams on its monotonic clock");
            seen.Add(stats.MaxForwardLatency.TotalMilliseconds.ToString("F4", System.Globalization.CultureInfo.InvariantCulture));
            if (stats.MaxForwardLatency < best) best = stats.MaxForwardLatency;
        }
        Assert.True(best < TimeSpan.FromMilliseconds(2),
            $"p100 receive-to-sink latency was at least 2 ms in each of {Replays} replays (ms: {string.Join(", ", seen)})");
    }

    [Fact]
    public void Garbage_on_the_socket_is_counted_and_ignored()
    {
        var sink = new RecordingSink();
        var core = new BridgeCore(MapBuilder.Standard().Build(), sink);
        using var host = new UdpBridgeHost(core, new IPEndPoint(IPAddress.Loopback, 0));
        host.Start();
        using var tx = Net.Sender(host.LocalEndPoint.Port);
        var rng = new Random(5);
        long sent = 0;
        void Send(byte[] d)
        {
            tx.Send(d, d.Length);
            sent++;
            Net.WaitUntil("the datagram to be received", () => host.Received >= sent);
        }

        Send(Array.Empty<byte>());
        for (int i = 0; i < 200; i++)
        {
            var junk = new byte[rng.Next(1, 1500)];
            rng.NextBytes(junk);
            Send(junk);
        }
        // Larger than any valid datagram (and than a typical receive buffer sized for 1456 bytes).
        Send(new byte[4000]);
        Send(new byte[20000]);
        Send(Wire.Delta(1, Wire.Co(3, true)));
        Net.WaitUntil("the valid datagram to be forwarded", () => sink.Count == 1);
        host.Stop();
        Assert.Equal(new[] { new Update('S', 1, 1) }, sink.Updates);
        Assert.Equal(1, core.Stats.Accepted);
        Assert.Equal(sent - 1, core.Stats.Rejected);
    }

    [Fact]
    public void Stop_is_idempotent_and_releases_the_port()
    {
        var core = new BridgeCore(MapBuilder.Standard().Build(), new RecordingSink());
        var host = new UdpBridgeHost(core, new IPEndPoint(IPAddress.Loopback, 0));
        host.Start();
        int port = host.LocalEndPoint.Port;
        Assert.True(Net.IsBound(port));
        host.Stop();
        host.Stop();
        host.Dispose();
        host.Dispose();
        Net.WaitUntil("the port to be released", () => !Net.IsBound(port));
    }

    [Fact]
    public void Dispose_without_stop_ends_the_receive_thread()
    {
        var sink = new RecordingSink();
        var core = new BridgeCore(MapBuilder.Standard().Build(), sink);
        int port;
        using (var host = new UdpBridgeHost(core, new IPEndPoint(IPAddress.Loopback, 0)))
        {
            host.Start();
            port = host.LocalEndPoint.Port;
        }
        Net.WaitUntil("the port to be released", () => !Net.IsBound(port));
        int before = sink.Calls.Count;
        using var tx = new UdpClient(AddressFamily.InterNetwork);
        var d = Wire.Delta(1, Wire.Co(3, true));
        tx.Send(d, d.Length, new IPEndPoint(IPAddress.Loopback, port));
        Thread.Sleep(50);
        Assert.Equal(before, sink.Calls.Count);
    }
}

[Collection("sockets")]
public class BridgeAppSpec
{
    private sealed class TempFiles : IDisposable
    {
        public readonly DirectoryInfo Dir = Directory.CreateTempSubdirectory("spec_dof_app_");

        public string Write(string name, string text)
        {
            var p = Path.Combine(Dir.FullName, name);
            File.WriteAllText(p, text);
            return p;
        }

        public void Dispose()
        {
            try
            {
                Dir.Delete(true);
            }
            catch (IOException)
            {
            }
        }
    }

    private static (int Code, string Out, string Err) Run(params string[] args)
    {
        var o = new StringWriter();
        var e = new StringWriter();
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(30));
        int code = BridgeApp.Run(args, o, e, new BridgeRunContext { SinkFactory = (_, _) => new RecordingSink(), Cancel = timeout.Token, Stdin = new BlockingReader() });
        Assert.False(timeout.IsCancellationRequested, "BridgeApp.Run did not return by itself");
        return (code, o.ToString(), e.ToString());
    }

    [Fact]
    public void Help_prints_the_options_and_exits_0()
    {
        var (code, stdout, stderr) = Run("--help");
        Assert.Equal(0, code);
        string text = stdout + stderr;
        foreach (var option in new[] { "--map", "--port", "--bind", "--rom", "--table", "--dof-path", "--sink", "--parent-pid", "--exit-on-stdin-eof" })
        {
            Assert.Contains(option, text, StringComparison.Ordinal);
        }
    }

    [Theory]
    [InlineData()]
    [InlineData("--port", "35120")]                              // --map is required
    [InlineData("--map")]                                        // value missing
    [InlineData("--map", "m.json", "--bogus")]
    [InlineData("--map", "m.json", "--port", "70000")]
    [InlineData("--map", "m.json", "--port", "-1")]
    [InlineData("--map", "m.json", "--port", "abc")]
    [InlineData("--map", "m.json", "--port")]
    [InlineData("--map", "m.json", "--sink", "printer")]
    [InlineData("--map", "m.json", "--bind", "not-an-ip")]
    [InlineData("--map", "m.json", "--parent-pid", "x")]
    [InlineData("--map", "m.json", "stray")]
    public void Usage_errors_exit_2(params string[] args)
    {
        Assert.Throws<BridgeUsageException>(() => BridgeOptions.Parse(args));
        var (code, _, stderr) = Run(args);
        Assert.Equal(2, code);
        Assert.NotEqual("", stderr.Trim());
    }

    [Fact]
    public void Valid_command_lines_parse()
    {
        Assert.NotNull(BridgeOptions.Parse(new[] { "--map", "m.json" }));
        Assert.NotNull(BridgeOptions.Parse(new[]
        {
            "--map", "C:\\Temp\\dof map.json", "--port", "40123", "--bind", "0.0.0.0", "--rom", "apexcircuit", "--table", "Apex Circuit",
            "--dof-path", "C:\\DirectOutput", "--dof-config", "C:\\DirectOutput\\config\\GlobalConfig_B2SServer.xml",
            "--sink", "log", "--log", "out.txt", "--parent-pid", "1234", "--exit-on-stdin-eof",
        }));
        foreach (var sink in new[] { "dof", "log", "null" })
        {
            Assert.NotNull(BridgeOptions.Parse(new[] { "--map", "m.json", "--sink", sink }));
        }
        Assert.NotNull(BridgeOptions.Parse(new[] { "--map", "m.json", "--port", "0" }));
        Assert.NotNull(BridgeOptions.Parse(new[] { "--map", "m.json", "--port", "65535" }));
    }

    [Fact]
    public void A_missing_or_invalid_map_file_exits_3()
    {
        using var tmp = new TempFiles();
        var (code, _, stderr) = Run("--map", Path.Combine(tmp.Dir.FullName, "nope.json"), "--sink", "null", "--port", "0");
        Assert.Equal(3, code);
        Assert.NotEqual("", stderr.Trim());

        var bad = tmp.Write("bad.json", "{ \"format\": \"kilohertz-dof-map\", \"version\": 99 }");
        (code, _, stderr) = Run("--map", bad, "--sink", "null", "--port", "0");
        Assert.Equal(3, code);
        Assert.NotEqual("", stderr.Trim());
    }

    [Fact]
    public void A_missing_dof_install_exits_4()
    {
        using var tmp = new TempFiles();
        var map = tmp.Write("map.json", MapBuilder.Standard().Json());
        var o = new StringWriter();
        var e = new StringWriter();
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(30));
        // Default sink is DOF itself; the given path holds no DirectOutput.dll.
        int code = BridgeApp.Run(new[] { "--map", map, "--port", "0", "--dof-path", tmp.Dir.FullName }, o, e, new BridgeRunContext { Cancel = timeout.Token });
        Assert.False(timeout.IsCancellationRequested);
        Assert.Equal(4, code);
        Assert.NotEqual("", e.ToString().Trim());
    }

    [Fact]
    public void A_port_in_use_exits_5()
    {
        using var tmp = new TempFiles();
        var map = tmp.Write("map.json", MapBuilder.Standard().Json());
        using var squatter = new Socket(AddressFamily.InterNetwork, SocketType.Dgram, ProtocolType.Udp);
        squatter.ExclusiveAddressUse = true;
        squatter.Bind(new IPEndPoint(IPAddress.Loopback, 0));
        int port = ((IPEndPoint)squatter.LocalEndPoint!).Port;
        var (code, _, stderr) = Run("--map", map, "--sink", "null", "--port", port.ToString(CultureInfo.InvariantCulture));
        Assert.Equal(5, code);
        Assert.NotEqual("", stderr.Trim());
    }

    private sealed class Running : IDisposable
    {
        public readonly CancellationTokenSource Cancel = new();
        public readonly StringWriter Out = new();
        public readonly StringWriter Err = new();
        public readonly Task<int> Task;
        public readonly int Port;
        public readonly BlockingReader Stdin = new();
        public volatile BridgeOptions? Options;
        public volatile ResolvedDofMap? Map;
        public volatile IPEndPoint? Listening;

        public Running(RecordingSink? sink, int port, params string[] args)
            : this(sink, port, null, args)
        {
        }

        public Running(RecordingSink? sink, int port, Func<int, bool>? processAlive, params string[] args)
        {
            Port = port;
            Func<BridgeOptions, ResolvedDofMap, IDofSink>? factory = sink is null
                ? null
                : (o, m) =>
                {
                    Options = o;
                    Map = m;
                    return sink;
                };
            var context = new BridgeRunContext
            {
                SinkFactory = factory,
                Stdin = Stdin,
                Cancel = Cancel.Token,
                Listening = ep => Listening = ep,
                ProcessAlive = processAlive,
                ParentPollInterval = TimeSpan.FromMilliseconds(20),
            };
            Task = System.Threading.Tasks.Task.Factory.StartNew(
                () => BridgeApp.Run(args, TextWriter.Synchronized(Out), TextWriter.Synchronized(Err), context),
                CancellationToken.None, TaskCreationOptions.LongRunning, TaskScheduler.Default);
            Net.WaitUntil("the bridge to listen or exit", () => Task.IsCompleted || Listening is not null);
            if (Listening is not null)
            {
                Assert.Equal(port, Listening.Port);
                Assert.True(Net.IsBound(port), "Listening was reported but the port is not bound");
            }
        }

        public int ExitCode() => Task.GetAwaiter().GetResult();

        public int Stop()
        {
            Cancel.Cancel();
            Net.WaitUntil("BridgeApp.Run to return after cancellation", () => Task.IsCompleted);
            return Task.GetAwaiter().GetResult();
        }

        public void Dispose()
        {
            Cancel.Cancel();
            Stdin.Close();
            try
            {
                Task.Wait(TimeSpan.FromSeconds(20));
            }
            catch (AggregateException)
            {
            }
            Cancel.Dispose();
        }
    }

    /// <summary>Sends the datagrams repeatedly until the condition holds (the bridge forwards changes only, so resends are harmless).</summary>
    private static void SendUntil(int port, IReadOnlyList<byte[]> datagrams, string what, Func<bool> done)
    {
        using var tx = Net.Sender(port);
        var deadline = Stopwatch.StartNew();
        while (!done())
        {
            if (deadline.Elapsed > TimeSpan.FromSeconds(20)) Assert.Fail($"timed out waiting for {what}");
            foreach (var d in datagrams) tx.Send(d, d.Length);
            Thread.Sleep(10);
        }
    }

    [Fact]
    public void Run_hosts_the_sink_from_init_to_finish()
    {
        using var tmp = new TempFiles();
        var map = tmp.Write("map.json", new MapBuilder { Rom = "apexcircuit", Table = "Apex Circuit" }.Add("coil", 3, 'S', 1).Add("lamp", 5, 'L', 30).Json());
        var sink = new RecordingSink();
        int port = Net.FreeUdpPort();
        using var app = new Running(sink, port, "--map", map, "--port", port.ToString(CultureInfo.InvariantCulture));
        Assert.False(app.Task.IsCompleted, $"exited early: {app.Err}");
        Assert.NotNull(app.Map);
        Assert.Equal(2, app.Map!.Entries.Count);

        var state = new CabinetState();
        state.Coils[3] = 1;
        state.Lamps[5] = 0.2f;
        SendUntil(port, state.FullState(0), "the full state to be forwarded", () => sink.Count >= 2);
        SendUntil(port, new[] { Wire.Delta(9, Wire.Co(3, false)) }, "the delta to be forwarded", () => sink.Count >= 3);

        Assert.Equal(0, app.Stop());
        Assert.Equal(new[] { new Update('S', 1, 1), new Update('L', 30, 51), new Update('S', 1, 0) }, sink.Updates);
        var calls = sink.Calls;
        Assert.Equal("Init(apexcircuit,Apex Circuit)", calls[0]);
        Assert.Equal("Finish", calls[^1]);
        Assert.Equal(1, calls.Count(c => c.StartsWith("Init", StringComparison.Ordinal)));
        Assert.Equal(1, calls.Count(c => c == "Finish"));
        Assert.True(Net.IsBound(port) == false, "the port is released when Run returns");
    }

    [Fact]
    public void Rom_and_table_options_override_the_map_file()
    {
        using var tmp = new TempFiles();
        var map = tmp.Write("map.json", new MapBuilder { Rom = "maprom", Table = "Map Table" }.Add("coil", 3, 'S', 1).Json());
        var sink = new RecordingSink();
        int port = Net.FreeUdpPort();
        using (var app = new Running(sink, port, "--map", map, "--port", port.ToString(CultureInfo.InvariantCulture), "--rom", "cli_rom", "--table", "CLI Table"))
        {
            Assert.Equal(0, app.Stop());
        }
        Assert.Equal(new[] { "Init(cli_rom,CLI Table)", "Finish" }, sink.Calls);

        sink = new RecordingSink();
        port = Net.FreeUdpPort();
        using (var app = new Running(sink, port, "--map", map, "--port", port.ToString(CultureInfo.InvariantCulture), "--rom", "only_rom"))
        {
            Assert.Equal(0, app.Stop());
        }
        Assert.Equal(new[] { "Init(only_rom,Map Table)", "Finish" }, sink.Calls);
    }

    [Fact]
    public void Log_sink_writes_one_line_per_update()
    {
        using var tmp = new TempFiles();
        var map = tmp.Write("map.json", MapBuilder.Standard().Json());
        var log = Path.Combine(tmp.Dir.FullName, "updates.log");
        var recording = Fixtures.Recording("session.bin");
        var expected = Fixtures.Expected("session.expected.txt");
        int port = Net.FreeUdpPort();
        using var app = new Running(null, port, "--map", map, "--port", port.ToString(CultureInfo.InvariantCulture), "--sink", "log", "--log", log);
        Assert.False(app.Task.IsCompleted, $"exited early: {app.Err}");

        using (var tx = Net.Sender(port))
        {
            foreach (var d in recording)
            {
                tx.Send(d, d.Length);
                Thread.Sleep(2);    // one sim tick apart at least, as recorded
            }
        }
        // The last update of the session; the sink may buffer, so also accept it only after shutdown.
        var settle = Stopwatch.StartNew();
        while (settle.Elapsed < TimeSpan.FromSeconds(3) && ReadLines(log).Count < expected.Count)
        {
            Thread.Sleep(10);
        }
        Assert.Equal(0, app.Stop());
        Assert.Equal(expected.Select(u => u.ToString()).ToList(), ReadLines(log));
    }

    private static List<string> ReadLines(string path)
    {
        if (!File.Exists(path)) return new List<string>();
        try
        {
            using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
            using var reader = new StreamReader(stream);
            return reader.ReadToEnd().Split('\n').Select(l => l.TrimEnd('\r')).Where(l => l.Length > 0 && l[0] != '#').ToList();
        }
        catch (IOException)
        {
            return new List<string>();
        }
    }

    [Fact]
    public void Log_sink_without_a_file_writes_to_stdout()
    {
        using var tmp = new TempFiles();
        var map = tmp.Write("map.json", new MapBuilder().Add("coil", 3, 'S', 1).Json());
        int port = Net.FreeUdpPort();
        using var app = new Running(null, port, "--map", map, "--port", port.ToString(CultureInfo.InvariantCulture), "--sink", "log");
        Assert.False(app.Task.IsCompleted, $"exited early: {app.Err}");
        using (var tx = Net.Sender(port))
        {
            var on = Wire.Delta(5, Wire.Co(3, true));
            var settle = Stopwatch.StartNew();
            while (settle.Elapsed < TimeSpan.FromSeconds(3) && !app.Out.ToString().Contains("S 1 1", StringComparison.Ordinal))
            {
                tx.Send(on, on.Length);
                Thread.Sleep(10);
            }
        }
        Assert.Equal(0, app.Stop());
        var lines = app.Out.ToString().Split('\n').Select(l => l.Trim()).ToList();
        Assert.Equal(1, lines.Count(l => l == "S 1 1"));
    }

    [Fact]
    public void Null_sink_runs_without_dof_and_stops_on_cancel()
    {
        using var tmp = new TempFiles();
        var map = tmp.Write("map.json", MapBuilder.Standard().Json());
        int port = Net.FreeUdpPort();
        using var app = new Running(null, port, "--map", map, "--port", port.ToString(CultureInfo.InvariantCulture), "--sink", "null");
        Assert.False(app.Task.IsCompleted, $"exited early: {app.Err}");
        using (var tx = Net.Sender(port))
        {
            foreach (var d in new CabinetState().FullState(0)) tx.Send(d, d.Length);
            tx.Send(new byte[] { 1, 2, 3 }, 3);
        }
        Assert.Equal(0, app.Stop());
    }

    [Fact]
    public void Parent_pid_of_a_dead_process_ends_the_bridge()
    {
        using var tmp = new TempFiles();
        var map = tmp.Write("map.json", MapBuilder.Standard().Json());
        int deadPid;
        var psi = OperatingSystem.IsWindows()
            ? new ProcessStartInfo("cmd.exe", "/c exit 0")
            : new ProcessStartInfo("/bin/sh", "-c \"exit 0\"");
        psi.UseShellExecute = false;
        psi.CreateNoWindow = true;
        using (var child = Process.Start(psi)!)
        {
            child.WaitForExit();
            deadPid = child.Id;
        }
        var sink = new RecordingSink();
        var o = new StringWriter();
        var e = new StringWriter();
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(30));
        int port = Net.FreeUdpPort();
        // The real process check (no ProcessAlive override).
        int code = BridgeApp.Run(
            new[] { "--map", map, "--port", port.ToString(CultureInfo.InvariantCulture), "--parent-pid", deadPid.ToString(CultureInfo.InvariantCulture) },
            o, e, new BridgeRunContext { SinkFactory = (_, _) => sink, Cancel = timeout.Token, Stdin = new BlockingReader(), ParentPollInterval = TimeSpan.FromMilliseconds(20) });
        Assert.False(timeout.IsCancellationRequested, "the bridge kept running although its parent is gone");
        Assert.Equal(0, code);
        // A sink that was initialised is also finished.
        var calls = sink.Calls;
        Assert.Equal(calls.Count(c => c.StartsWith("Init", StringComparison.Ordinal)), calls.Count(c => c == "Finish"));
    }

    [Fact]
    public void Parent_pid_of_a_live_process_keeps_the_bridge_running_until_it_exits()
    {
        using var tmp = new TempFiles();
        var map = tmp.Write("map.json", new MapBuilder().Add("coil", 3, 'S', 1).Json());
        var sink = new RecordingSink();
        int port = Net.FreeUdpPort();
        int alive = 1;
        int askedFor = -1;
        using var app = new Running(sink, port, pid =>
            {
                askedFor = pid;
                return Volatile.Read(ref alive) == 1;
            },
            "--map", map, "--port", port.ToString(CultureInfo.InvariantCulture), "--parent-pid", "4242");
        SendUntil(port, new[] { Wire.Delta(1, Wire.Co(3, true)) }, "the delta to be forwarded", () => sink.Count >= 1);
        Net.WaitUntil("the parent to be polled", () => Volatile.Read(ref askedFor) == 4242);
        Assert.False(app.Task.IsCompleted);

        // The engine dies without closing anything: the bridge notices and shuts DOF down.
        Volatile.Write(ref alive, 0);
        Net.WaitUntil("the bridge to exit after its parent", () => app.Task.IsCompleted);
        Assert.Equal(0, app.ExitCode());
        Assert.Equal("Finish", sink.Calls[^1]);
        Net.WaitUntil("the port to be released", () => !Net.IsBound(port));
    }

    [Fact]
    public void Real_parent_pid_keeps_the_bridge_running()
    {
        using var tmp = new TempFiles();
        var map = tmp.Write("map.json", new MapBuilder().Add("coil", 3, 'S', 1).Json());
        var sink = new RecordingSink();
        int port = Net.FreeUdpPort();
        using var app = new Running(sink, port, "--map", map, "--port", port.ToString(CultureInfo.InvariantCulture),
            "--parent-pid", Environment.ProcessId.ToString(CultureInfo.InvariantCulture));
        SendUntil(port, new[] { Wire.Delta(1, Wire.Co(3, true)) }, "the delta to be forwarded", () => sink.Count >= 1);
        Thread.Sleep(200);  // ten poll intervals
        Assert.False(app.Task.IsCompleted, "the bridge exited although its parent is alive");
        Assert.Equal(0, app.Stop());
    }

    /// <summary>The engine's stop signal: stdin closes → clean shutdown (DOF Finish), exit 0.</summary>
    [Fact]
    public void Exit_on_stdin_eof_shuts_down_cleanly()
    {
        using var tmp = new TempFiles();
        var map = tmp.Write("map.json", new MapBuilder().Add("coil", 3, 'S', 1).Json());
        var sink = new RecordingSink();
        int port = Net.FreeUdpPort();
        using var app = new Running(sink, port, "--map", map, "--port", port.ToString(CultureInfo.InvariantCulture), "--exit-on-stdin-eof");
        SendUntil(port, new[] { Wire.Delta(1, Wire.Co(3, true)) }, "the delta to be forwarded", () => sink.Count >= 1);
        Thread.Sleep(100);
        Assert.False(app.Task.IsCompleted, "the bridge exited while stdin was still open");

        app.Stdin.Close();
        Net.WaitUntil("the bridge to exit after stdin closed", () => app.Task.IsCompleted);
        Assert.Equal(0, app.ExitCode());
        Assert.Equal(new[] { "Init(specrom,Spec Table)", "ReceiveData", "Finish" }, sink.Calls);
        Net.WaitUntil("the port to be released", () => !Net.IsBound(port));
    }

    [Fact]
    public void Stdin_eof_is_ignored_without_the_option()
    {
        using var tmp = new TempFiles();
        var map = tmp.Write("map.json", new MapBuilder().Add("coil", 3, 'S', 1).Json());
        var sink = new RecordingSink();
        int port = Net.FreeUdpPort();
        using var app = new Running(sink, port, "--map", map, "--port", port.ToString(CultureInfo.InvariantCulture));
        app.Stdin.Close();
        SendUntil(port, new[] { Wire.Delta(1, Wire.Co(3, true)) }, "the delta to be forwarded", () => sink.Count >= 1);
        Thread.Sleep(100);
        Assert.False(app.Task.IsCompleted, "the bridge exited on stdin EOF without --exit-on-stdin-eof");
        Assert.Equal(0, app.Stop());
    }
}

/// <summary>A stdin stand-in that stays open (reads block) until <see cref="Close"/> signals end of input.</summary>
public sealed class BlockingReader : TextReader
{
    private readonly ManualResetEventSlim _closed = new(false);

    public override void Close() => _closed.Set();

    public override int Read()
    {
        _closed.Wait();
        return -1;
    }

    public override int Peek()
    {
        _closed.Wait();
        return -1;
    }

    public override int Read(char[] buffer, int index, int count)
    {
        _closed.Wait();
        return 0;
    }

    public override int Read(Span<char> buffer)
    {
        _closed.Wait();
        return 0;
    }

    public override string? ReadLine()
    {
        _closed.Wait();
        return null;
    }

    public override string ReadToEnd()
    {
        _closed.Wait();
        return string.Empty;
    }

    public override Task<int> ReadAsync(char[] buffer, int index, int count) => Task.Run(() => Read(buffer, index, count));

    public override ValueTask<int> ReadAsync(Memory<char> buffer, CancellationToken cancellationToken = default) =>
        new(Task.Run(() =>
        {
            _closed.Wait(cancellationToken);
            return 0;
        }, cancellationToken));

    public override Task<string?> ReadLineAsync() => Task.Run(ReadLine);

    public override ValueTask<string?> ReadLineAsync(CancellationToken cancellationToken) =>
        new(Task.Run(() =>
        {
            _closed.Wait(cancellationToken);
            return (string?)null;
        }, cancellationToken));

    public override Task<string> ReadToEndAsync() => Task.Run(ReadToEnd);

    public override Task<string> ReadToEndAsync(CancellationToken cancellationToken) =>
        Task.Run(() =>
        {
            _closed.Wait(cancellationToken);
            return string.Empty;
        }, cancellationToken);
}

public class DofLocatorSpec
{
    [Fact]
    public void An_explicit_file_or_directory_is_used()
    {
        var dir = Directory.CreateTempSubdirectory("spec_dof_loc_");
        try
        {
            // Nothing there: not found (an explicit path that is wrong must not fall through silently to
            // "found"; whether other locations are searched is up to the locator, so only assert when
            // the machine has no DOF at all).
            string dll = Path.Combine(dir.FullName, "DirectOutput.dll");
            File.WriteAllBytes(dll, new byte[] { 0x4D, 0x5A });
            Assert.Equal(Path.GetFullPath(dll), Path.GetFullPath(DofLocator.FindDirectOutputDll(dll)!));
            Assert.Equal(Path.GetFullPath(dll), Path.GetFullPath(DofLocator.FindDirectOutputDll(dir.FullName)!));
        }
        finally
        {
            dir.Delete(true);
        }
    }

    [Fact]
    public void Loading_something_that_is_not_directoutput_fails_cleanly()
    {
        var dir = Directory.CreateTempSubdirectory("spec_dof_load_");
        try
        {
            string dll = Path.Combine(dir.FullName, "DirectOutput.dll");
            File.WriteAllBytes(dll, new byte[] { 0x4D, 0x5A, 0, 0 });
            var ex = Record.Exception(() => ReflectionDofSink.Load(dll, null));
            Assert.NotNull(ex);
            Assert.IsNotType<NullReferenceException>(ex);
            Assert.IsNotType<AccessViolationException>(ex);
        }
        finally
        {
            dir.Delete(true);
        }
    }
}
