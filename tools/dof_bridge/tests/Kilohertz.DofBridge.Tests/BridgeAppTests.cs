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

    /// <summary>Review N4: a sink that throws on the receive thread ends the bridge (exit 6) instead of
    /// leaving it alive but deaf, and DOF <c>Finish</c> is still called.</summary>
    [Fact]
    public void ThrowingSinkEndsTheBridgeWithFinish()
    {
        var sink = new ThrowingSink();
        using var listening = new ManualResetEventSlim();
        IPEndPoint? ep = null;
        var ctx = new BridgeRunContext { SinkFactory = (_, _) => sink, Listening = e => { ep = e; listening.Set(); } };
        var run = new Running(["--map", MapPath, "--port", "0"], ctx, new StringWriter(), new StringWriter());
        Assert.True(listening.Wait(Timeout));
        using (Socket tx = Sender(ep!))
        {
            tx.Send(Dg.Make(0, true, Dg.Coil(1, 1)));
        }
        Assert.Equal(BridgeApp.ExitFault, run.Exit());
        Assert.True(sink.Finished);
        Assert.Contains("stopped receiving: IOException: toy on fire", run.Stderr.ToString());
    }

    [Fact]
    public void ThrowingFinishIsReportedAsFault()
    {
        var sink = new ThrowingSink { ThrowOnReceive = false, ThrowOnFinish = true };
        using var cts = new CancellationTokenSource();
        var ctx = new BridgeRunContext { SinkFactory = (_, _) => sink, Cancel = cts.Token, Listening = _ => cts.Cancel() };
        var run = new Running(["--map", MapPath, "--port", "0"], ctx, new StringWriter(), new StringWriter());
        Assert.Equal(BridgeApp.ExitFault, run.Exit());
        Assert.Contains("DOF Finish failed", run.Stderr.ToString());
    }

    [Fact]
    public void HostReportsFaultOnce()
    {
        var core = new BridgeCore(Fixtures.Map(), new ThrowingSink());
        Exception? seen = null;
        using var faulted = new ManualResetEventSlim();
        using var host = new UdpBridgeHost(core, new IPEndPoint(IPAddress.Loopback, 0), e => { seen = e; faulted.Set(); });
        host.Start();
        using (Socket tx = Sender(host.LocalEndPoint))
        {
            tx.Send(Dg.Make(0, true, Dg.Coil(1, 1)));
        }
        Assert.True(faulted.Wait(Timeout));
        Assert.IsType<IOException>(seen);
        Assert.Same(seen, host.Fault);
        host.Stop();
    }

    /// <summary>Stopping the host from its own fault callback (on the receive thread) must not join
    /// that thread; an external Stop afterwards still waits for it.</summary>
    [Fact]
    public void StopFromFaultCallbackDoesNotDeadlock()
    {
        var core = new BridgeCore(Fixtures.Map(), new ThrowingSink());
        using var callbackDone = new ManualResetEventSlim();
        UdpBridgeHost? host = null;
        Thread? faultThread = null;
        host = new UdpBridgeHost(core, new IPEndPoint(IPAddress.Loopback, 0), _ =>
        {
            faultThread = Thread.CurrentThread;
            host!.Stop();
            callbackDone.Set();
        });
        using (host)
        {
            host.Start();
            using (Socket tx = Sender(host.LocalEndPoint))
            {
                tx.Send(Dg.Make(0, true, Dg.Coil(1, 1)));
            }
            Assert.True(callbackDone.Wait(Timeout), "Stop() on the receive thread deadlocked");
            host.Stop();
            Assert.False(faultThread!.IsAlive);
            Assert.IsType<IOException>(host.Fault);
        }
    }

    /// <summary>The bridge does not authenticate senders, so it only listens on loopback.</summary>
    [Theory]
    [InlineData("0.0.0.0")]
    [InlineData("192.168.1.20")]
    [InlineData("::")]
    [InlineData("::ffff:10.0.0.1")]
    public void NonLoopbackBindIsRejected(string bind)
    {
        var err = new StringWriter();
        var ctx = new BridgeRunContext { SinkFactory = (_, _) => throw new InvalidOperationException("sink must not be created") };
        Assert.Equal(BridgeApp.ExitUsage, BridgeApp.Run(["--map", MapPath, "--port", "0", "--bind", bind], new StringWriter(), err, ctx));
        Assert.Contains("not a loopback address", err.ToString());
    }

    [Theory]
    [InlineData("127.0.0.1", true)]
    [InlineData("127.1.2.3", true)]
    [InlineData("::1", true)]
    [InlineData("::ffff:127.0.0.1", true)]
    [InlineData("0.0.0.0", false)]
    [InlineData("::", false)]
    [InlineData("10.0.0.1", false)]
    public void LoopbackBindPredicate(string address, bool loopback) =>
        Assert.Equal(loopback, BridgeApp.IsLoopbackBind(IPAddress.Parse(address)));

    /// <summary>Review N5: reflection failures while loading DOF fail closed with exit 4, no stack trace.</summary>
    [Theory]
    [InlineData(typeof(MissingMethodException))]
    [InlineData(typeof(MissingFieldException))]
    [InlineData(typeof(TypeLoadException))]
    [InlineData(typeof(ArgumentException))]
    [InlineData(typeof(MemberAccessException))]
    [InlineData(typeof(FileLoadException))]
    [InlineData(typeof(BadImageFormatException))]
    public void ReflectionFailuresExit4(Type exception)
    {
        var ctx = new BridgeRunContext { SinkFactory = (_, _) => throw (Exception)Activator.CreateInstance(exception, "bad dof")! };
        var err = new StringWriter();
        Assert.Equal(BridgeApp.ExitDof, BridgeApp.Run(["--map", MapPath, "--port", "0"], new StringWriter(), err, ctx));
        Assert.DoesNotContain("   at ", err.ToString());
        var rtl = new BridgeRunContext { SinkFactory = (_, _) => throw new System.Reflection.ReflectionTypeLoadException([], []) };
        Assert.Equal(BridgeApp.ExitDof, BridgeApp.Run(["--map", MapPath, "--port", "0"], new StringWriter(), new StringWriter(), rtl));
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
        try
        {
            string? line = ReadLineWithin(p.StandardError);
            Assert.NotNull(line);
            Match m = Regex.Match(line!, @"listening on [\d.]+:(\d+)");
            Assert.True(m.Success, line);
            int port = int.Parse(m.Groups[1].Value, System.Globalization.CultureInfo.InvariantCulture);
            using (Socket tx = Sender(new IPEndPoint(IPAddress.Loopback, port)))
            {
                tx.Send(Dg.Make(0, false, Dg.Coil(1, 1)));
            }
            Assert.Equal("# init rom=dof_fixture_rom table=DOF \"Fixture\" Table", ReadLineWithin(p.StandardOutput));
            Assert.Equal("S 1 1", ReadLineWithin(p.StandardOutput));
            p.StandardInput.Close();
            Assert.True(p.WaitForExit(20_000));
            Assert.Equal(0, p.ExitCode);
            Assert.Equal("# finish", ReadLineWithin(p.StandardOutput));
        }
        finally
        {
            // A failed or timed-out step must not leave the bridge running.
            if (!p.HasExited)
            {
                p.Kill(entireProcessTree: true);
                p.WaitForExit(20_000);
            }
        }
    }

    /// <summary>The next line of <paramref name="reader"/>, failing the test after <see cref="Timeout"/>
    /// instead of blocking forever when the child never writes it.</summary>
    private static string? ReadLineWithin(StreamReader reader)
    {
        Task<string?> read = reader.ReadLineAsync();
        Assert.True(read.Wait(Timeout), "timed out waiting for a line from the bridge");
        return read.Result;
    }

    private sealed class ThrowingSink : IDofSink
    {
        public bool ThrowOnReceive { get; init; } = true;
        public bool ThrowOnFinish { get; init; }
        public bool Finished { get; private set; }

        public void Init(string romName, string tableName)
        {
        }

        public void ReceiveData(char tableElementType, int number, int value)
        {
            if (ThrowOnReceive)
            {
                throw new IOException("toy on fire");
            }
        }

        public void Finish()
        {
            Finished = true;
            if (ThrowOnFinish)
            {
                throw new InvalidOperationException("finish failed");
            }
        }
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
