using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using System.Text.RegularExpressions;
using Xunit;

namespace Kilohertz.DofBridge.Tests;

public class BridgeAppTests
{
    private static readonly string MapPath = Path.Combine(Fixtures.Dir, "map.json");
    private static readonly TimeSpan Timeout = TimeSpan.FromSeconds(20);

    private sealed class Running
    {
        public Running(string[] args, BridgeRunContext ctx, StringWriter stdout, StringWriter stderr)
        {
            Stdout = stdout;
            Stderr = stderr;
            Task = System.Threading.Tasks.Task.Factory.StartNew(
                () => BridgeApp.Run(args, stdout, stderr, ctx), TaskCreationOptions.LongRunning);
        }

        public Task<int> Task { get; }
        public StringWriter Stdout { get; }
        public StringWriter Stderr { get; }

        public int Exit()
        {
            Assert.True(Task.Wait(Timeout), "bridge did not stop");
            return Task.Result;
        }
    }

    private static int RunSync(params string[] args)
    {
        var o = new StringWriter();
        var e = new StringWriter();
        return BridgeApp.Run(args, o, e);
    }

    private static Socket Sender(IPEndPoint ep)
    {
        var s = new Socket(AddressFamily.InterNetwork, SocketType.Dgram, ProtocolType.Udp);
        s.Connect(ep);
        return s;
    }

    [Fact]
    public void HelpAndUsageErrors()
    {
        var o = new StringWriter();
        Assert.Equal(BridgeApp.ExitOk, BridgeApp.Run(["--help"], o, new StringWriter()));
        Assert.Contains("--map", o.ToString());
        Assert.Equal(BridgeApp.ExitUsage, RunSync());
        Assert.Equal(BridgeApp.ExitUsage, RunSync("--map"));
        Assert.Equal(BridgeApp.ExitUsage, RunSync("--map", MapPath, "--bogus"));
        Assert.Equal(BridgeApp.ExitMap, RunSync("--map", Path.Combine(Fixtures.Dir, "missing.json"), "--sink", "null"));
    }

    [Fact]
    public void DofNotFoundExits4()
    {
        Assert.Equal(BridgeApp.ExitDof, RunSync("--map", MapPath, "--port", "0", "--dof-path", Path.Combine(TempDir.Create(), "none")));
        var ctx = new BridgeRunContext { SinkFactory = (_, _) => throw new InvalidOperationException("boom") };
        Assert.Equal(BridgeApp.ExitDof, BridgeApp.Run(["--map", MapPath, "--port", "0"], new StringWriter(), new StringWriter(), ctx));
    }

    [Fact]
    public void BindFailureExits5()
    {
        using var taken = new Socket(AddressFamily.InterNetwork, SocketType.Dgram, ProtocolType.Udp);
        taken.Bind(new IPEndPoint(IPAddress.Loopback, 0));
        int port = ((IPEndPoint)taken.LocalEndPoint!).Port;
        var sink = new RecordingSink();
        var ctx = new BridgeRunContext { SinkFactory = (_, _) => sink };
        Assert.Equal(BridgeApp.ExitSocket, BridgeApp.Run(["--map", MapPath, "--port", port.ToString(System.Globalization.CultureInfo.InvariantCulture)], new StringWriter(), new StringWriter(), ctx));
        Assert.True(sink.Finished);
    }

    [Fact]
    public void ForwardsUntilCancelled()
    {
        var sink = new RecordingSink();
        using var cts = new CancellationTokenSource();
        using var listening = new ManualResetEventSlim();
        IPEndPoint? ep = null;
        var ctx = new BridgeRunContext
        {
            SinkFactory = (_, _) => sink,
            Cancel = cts.Token,
            Listening = e => { ep = e; listening.Set(); },
        };
        var run = new Running(["--map", MapPath, "--port", "0", "--rom", "override_rom"], ctx, new StringWriter(), new StringWriter());
        Assert.True(listening.Wait(Timeout));
        using (Socket tx = Sender(ep!))
        {
            tx.Send(Dg.Make(0, true, Dg.Coil(1, 1), Dg.Gi(2, 1)));
        }
        Assert.True(SpinWait.SpinUntil(() => sink.Snapshot().Length >= 2, Timeout));
        cts.Cancel();
        Assert.Equal(0, run.Exit());
        Assert.Equal(["S 1 1", "G 2 255"], sink.Snapshot());
        Assert.Equal("override_rom|DOF \"Fixture\" Table", sink.InitArgs);
        Assert.True(sink.Finished);
        Assert.Contains("forwarded=2", run.Stderr.ToString());
        Assert.Contains("unverified on hardware", run.Stderr.ToString());
    }

    [Fact]
    public void LogSinkToFileStopsOnStdinEof()
    {
        string log = Path.Combine(TempDir.Create(), "updates.log");
        using var listening = new ManualResetEventSlim();
        IPEndPoint? ep = null;
        var stdin = new BlockingReader();
        var ctx = new BridgeRunContext { Stdin = stdin, Listening = e => { ep = e; listening.Set(); } };
        var run = new Running(["--map", MapPath, "--port", "0", "--bind", "127.0.0.1", "--sink", "log", "--log", log, "--exit-on-stdin-eof", "--table", "T"], ctx, new StringWriter(), new StringWriter());
        Assert.True(listening.Wait(Timeout));
        using (Socket tx = Sender(ep!))
        {
            tx.Send(Dg.Make(0, false, Dg.Lamp(40, 0.5f)));
        }
        stdin.WaitUntilRead();
        stdin.Close();
        Assert.Equal(0, run.Exit());
        string text = File.ReadAllText(log);
        Assert.StartsWith("# init rom=dof_fixture_rom table=T", text, StringComparison.Ordinal);
        Assert.EndsWith("# finish" + Environment.NewLine, text, StringComparison.Ordinal);
    }

    [Fact]
    public void LogSinkToStdoutAndNullSink()
    {
        using var cts = new CancellationTokenSource();
        var run = new Running(["--map", MapPath, "--port", "0", "--sink", "log"], new BridgeRunContext { Cancel = cts.Token, Listening = _ => cts.Cancel() }, new StringWriter(), new StringWriter());
        Assert.Equal(0, run.Exit());
        Assert.Contains("# init rom=dof_fixture_rom", run.Stdout.ToString());

        using var cts2 = new CancellationTokenSource();
        var run2 = new Running(["--map", MapPath, "--port", "0", "--sink", "null"], new BridgeRunContext { Cancel = cts2.Token, Listening = _ => cts2.Cancel() }, new StringWriter(), new StringWriter());
        Assert.Equal(0, run2.Exit());
    }

    [Fact]
    public void ExitsWhenParentIsGone()
    {
        int checks = 0;
        var ctx = new BridgeRunContext
        {
            SinkFactory = (_, _) => new NullDofSink(),
            ProcessAlive = pid => Interlocked.Increment(ref checks) < 3,
            ParentPollInterval = TimeSpan.FromMilliseconds(1),
        };
        var run = new Running(["--map", MapPath, "--port", "0", "--parent-pid", "4242"], ctx, new StringWriter(), new StringWriter());
        Assert.Equal(0, run.Exit());
        Assert.True(checks >= 3);
    }

    [Fact]
    public void ProcessAliveChecksTheOs()
    {
        Assert.True(BridgeApp.IsProcessAlive(Environment.ProcessId));
        Assert.False(BridgeApp.IsProcessAlive(int.MaxValue));
    }

    [Fact]
    public void OptionsParse()
    {
        BridgeOptions o = BridgeOptions.Parse(["--map", "m.json", "--port", "40000", "--bind", "0.0.0.0", "--rom", "r", "--table", "t",
            "--dof-path", "C:/DOF", "--dof-config", "g.xml", "--sink", "log", "--log", "l.txt", "--parent-pid", "12", "--exit-on-stdin-eof"]);
        Assert.Equal("m.json", o.MapFile);
        Assert.Equal(40000, o.Port);
        Assert.Equal(IPAddress.Any, o.Bind);
        Assert.Equal("r", o.RomName);
        Assert.Equal("t", o.TableName);
        Assert.Equal("C:/DOF", o.DofPath);
        Assert.Equal("g.xml", o.DofConfig);
        Assert.Equal(SinkKind.Log, o.Sink);
        Assert.Equal("l.txt", o.LogFile);
        Assert.Equal(12, o.ParentPid);
        Assert.True(o.ExitOnStdinEof);
        Assert.False(o.Help);

        BridgeOptions d = BridgeOptions.Parse(["--map", "m.json"]);
        Assert.Equal(BridgeOptions.DefaultPort, d.Port);
        Assert.Equal(IPAddress.Loopback, d.Bind);
        Assert.Equal(SinkKind.Dof, d.Sink);
        Assert.Null(d.ParentPid);
        Assert.True(BridgeOptions.Parse(["-h"]).Help);
    }

    [Theory]
    [InlineData("--map", "m", "--port", "65536")]
    [InlineData("--map", "m", "--port", "-1")]
    [InlineData("--map", "m", "--port", "x")]
    [InlineData("--map", "m", "--bind", "localhost")]
    [InlineData("--map", "m", "--sink", "dmx")]
    [InlineData("--map", "m", "--parent-pid", "0")]
    [InlineData("--port", "1")]
    [InlineData("--map", "m", "--rom")]
    public void OptionsRejectBadInput(params string[] args) =>
        Assert.Throws<BridgeUsageException>(() => BridgeOptions.Parse(args));

    /// <summary>The real executable, as the engine launches it: stdin pipe, stop by closing stdin.</summary>
    [Fact]
    public void ExecutableRunsAndStopsOnStdinEof()
    {
        string dll = Path.Combine(AppContext.BaseDirectory, "KilohertzDofBridge.dll");
        string dotnet = Environment.GetEnvironmentVariable("DOTNET_HOST_PATH") is string h && File.Exists(h) ? h : "dotnet";
        var psi = new ProcessStartInfo(dotnet)
        {
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
        };
        foreach (string a in new[] { dll, "--map", MapPath, "--port", "0", "--sink", "log", "--exit-on-stdin-eof", "--parent-pid", Environment.ProcessId.ToString(System.Globalization.CultureInfo.InvariantCulture) })
        {
            psi.ArgumentList.Add(a);
        }
        using Process p = Process.Start(psi)!;
        string? line = p.StandardError.ReadLine();
        Assert.NotNull(line);
        Match m = Regex.Match(line!, @"listening on [\d.]+:(\d+)");
        Assert.True(m.Success, line);
        int port = int.Parse(m.Groups[1].Value, System.Globalization.CultureInfo.InvariantCulture);
        using (Socket tx = Sender(new IPEndPoint(IPAddress.Loopback, port)))
        {
            tx.Send(Dg.Make(0, false, Dg.Coil(1, 1)));
        }
        Assert.Equal("# init rom=dof_fixture_rom table=DOF \"Fixture\" Table", p.StandardOutput.ReadLine());
        Assert.Equal("S 1 1", p.StandardOutput.ReadLine());
        p.StandardInput.Close();
        Assert.True(p.WaitForExit(20_000));
        Assert.Equal(0, p.ExitCode);
        Assert.Equal("# finish", p.StandardOutput.ReadLine());
    }

    /// <summary>A stdin whose end of file the test controls.</summary>
    private sealed class BlockingReader : TextReader
    {
        private readonly ManualResetEventSlim _closed = new();
        private readonly ManualResetEventSlim _reading = new();

        public void WaitUntilRead() => Assert.True(_reading.Wait(Timeout));

        public override int Read()
        {
            _reading.Set();
            _closed.Wait();
            return -1;
        }

        public override void Close() => _closed.Set();
    }
}
