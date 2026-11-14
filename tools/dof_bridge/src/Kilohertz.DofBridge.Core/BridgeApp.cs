using System.Diagnostics;
using System.Globalization;
using System.Net;
using System.Net.Sockets;
using System.Reflection;

namespace Kilohertz.DofBridge;

/// <summary>Optional hooks for <see cref="BridgeApp.Run"/> (tests and embedding).</summary>
public sealed class BridgeRunContext
{
    /// <summary>Creates the sink instead of the <c>--sink</c> choice.</summary>
    public Func<BridgeOptions, ResolvedDofMap, IDofSink>? SinkFactory { get; init; }
    /// <summary>Stdin watched by <c>--exit-on-stdin-eof</c> (default <see cref="Console.In"/>).</summary>
    public TextReader? Stdin { get; init; }
    /// <summary>Stops the bridge when cancelled.</summary>
    public CancellationToken Cancel { get; init; }
    /// <summary>Called with the bound endpoint once the bridge listens.</summary>
    public Action<IPEndPoint>? Listening { get; init; }
    /// <summary>Whether a process id is alive (for <c>--parent-pid</c>; default asks the OS).</summary>
    public Func<int, bool>? ProcessAlive { get; init; }
    /// <summary>How often <c>--parent-pid</c> is checked (default 250 ms).</summary>
    public TimeSpan ParentPollInterval { get; init; } = TimeSpan.FromMilliseconds(250);
}

/// <summary>The bridge process: options → map → sink → UDP host, until a stop signal.</summary>
public static class BridgeApp
{
    /// <summary>Exit code: clean shutdown (or <c>--help</c>).</summary>
    public const int ExitOk = 0;
    /// <summary>Exit code: bad command line.</summary>
    public const int ExitUsage = 2;
    /// <summary>Exit code: map file missing or invalid.</summary>
    public const int ExitMap = 3;
    /// <summary>Exit code: DOF not found or failed to load.</summary>
    public const int ExitDof = 4;
    /// <summary>Exit code: socket bind failed.</summary>
    public const int ExitSocket = 5;
    /// <summary>Exit code: the bridge failed while running (the receive thread died, e.g. the DOF
    /// sink threw, or an unexpected socket error); DOF <c>Finish</c> was still called.</summary>
    public const int ExitFault = 6;

    /// <summary>Runs the bridge until stdin EOF (<c>--exit-on-stdin-eof</c>), the parent exits
    /// (<c>--parent-pid</c>), Ctrl+C, or <see cref="BridgeRunContext.Cancel"/>.</summary>
    /// <param name="args">Command line.</param>
    /// <param name="stdout">Standard output (log sink default target).</param>
    /// <param name="stderr">Diagnostics.</param>
    /// <param name="context">Optional hooks.</param>
    /// <returns>Process exit code.</returns>
    public static int Run(string[] args, TextWriter stdout, TextWriter stderr, BridgeRunContext? context = null)
    {
        ArgumentNullException.ThrowIfNull(args);
        ArgumentNullException.ThrowIfNull(stdout);
        ArgumentNullException.ThrowIfNull(stderr);
        context ??= new BridgeRunContext();
        BridgeOptions options;
        try
        {
            options = BridgeOptions.Parse(args);
        }
        catch (BridgeUsageException e)
        {
            stderr.WriteLine($"error: {e.Message}");
            stderr.WriteLine(BridgeOptions.Usage);
            return ExitUsage;
        }
        if (options.Help)
        {
            stdout.WriteLine(BridgeOptions.Usage);
            return ExitOk;
        }

        ResolvedDofMap map;
        try
        {
            map = ResolvedDofMap.Load(options.MapFile);
        }
        catch (DofMapException e)
        {
            stderr.WriteLine($"error: {e.Message}");
            return ExitMap;
        }
        string rom = options.RomName ?? map.RomName;
        string table = options.TableName ?? map.TableName;

        TextWriter? logFile = null;
        IDofSink sink;
        try
        {
            sink = CreateSink(options, map, stdout, context, ref logFile);
            sink.Init(rom, table);
        }
        catch (Exception e) when (IsDofLoadFailure(e))
        {
            stderr.WriteLine($"error: DOF could not be started: {e.GetType().Name}: {e.Message}");
            logFile?.Dispose();
            return ExitDof;
        }

        using var done = new ManualResetEventSlim(false);
        BridgeCore core = new(map, sink);
        UdpBridgeHost host;
        try
        {
            host = new UdpBridgeHost(core, new IPEndPoint(options.Bind, options.Port), _ => SetQuietly(done));
        }
        catch (SocketException e)
        {
            stderr.WriteLine($"error: cannot bind {options.Bind}:{options.Port}: {e.Message}");
            sink.Finish();
            logFile?.Dispose();
            return ExitSocket;
        }

        ConsoleCancelEventHandler onCtrlC = (_, ev) =>
        {
            ev.Cancel = true;
            done.Set();
        };
        Console.CancelKeyPress += onCtrlC;
        using CancellationTokenRegistration reg = context.Cancel.Register(done.Set);
        using Timer? parentWatch = options.ParentPid is int pid ? StartParentWatch(pid, context, done) : null;
        if (options.ExitOnStdinEof)
        {
            TextReader stdin = context.Stdin ?? Console.In;
            var reader = new Thread(() => WaitForEof(stdin, done)) { IsBackground = true, Name = "dof-bridge-stdin" };
            reader.Start();
        }

        int exit = ExitOk;
        try
        {
            host.Start();
            stderr.WriteLine(string.Create(CultureInfo.InvariantCulture,
                $"KilohertzDofBridge: rom={rom} entries={map.Entries.Count} listening on {host.LocalEndPoint} (DOF hosting unverified on hardware)"));
            context.Listening?.Invoke(host.LocalEndPoint);
            done.Wait();
        }
        finally
        {
            Console.CancelKeyPress -= onCtrlC;
            host.Dispose();
            if (host.Fault is Exception fault)
            {
                stderr.WriteLine($"error: the bridge stopped receiving: {fault.GetType().Name}: {fault.Message}");
                exit = ExitFault;
            }
            try
            {
                sink.Finish();
            }
            catch (Exception e)
            {
                stderr.WriteLine($"error: DOF Finish failed: {e.GetType().Name}: {e.Message}");
                exit = exit == ExitOk ? ExitFault : exit;
            }
            logFile?.Dispose();
        }
        BridgeStats s = core.Stats;
        stderr.WriteLine(string.Create(CultureInfo.InvariantCulture,
            $"KilohertzDofBridge: stopped; accepted={s.Accepted} rejected={s.Rejected} stale={s.Stale} forwarded={s.Forwarded} resyncs={s.Resyncs} max_latency_ms={s.MaxForwardLatency.TotalMilliseconds:F3}"));
        return exit;
    }

    /// <summary>Exceptions that mean "DOF could not be loaded or started" (exit 4, no stack trace).</summary>
    private static bool IsDofLoadFailure(Exception e) =>
        e is InvalidOperationException or IOException or BadImageFormatException or TargetInvocationException
            or UnauthorizedAccessException or NotSupportedException or MissingMemberException or TypeLoadException
            or ReflectionTypeLoadException or ArgumentException or MemberAccessException
            or System.Security.SecurityException;

    private static IDofSink CreateSink(BridgeOptions options, ResolvedDofMap map, TextWriter stdout, BridgeRunContext context, ref TextWriter? logFile)
    {
        if (context.SinkFactory != null)
        {
            return context.SinkFactory(options, map);
        }
        switch (options.Sink)
        {
            case SinkKind.Null:
                return new NullDofSink();
            case SinkKind.Log:
                if (options.LogFile != null)
                {
                    logFile = new StreamWriter(options.LogFile, append: false);
                    return new LogDofSink(logFile);
                }
                return new LogDofSink(stdout);
            default:
                string dll = DofLocator.FindDirectOutputDll(options.DofPath)
                    ?? throw new InvalidOperationException("DirectOutput.dll not found (use --dof-path or KHZ_DOF_PATH)");
                return ReflectionDofSink.Load(dll, options.DofConfig);
        }
    }

    private static Timer StartParentWatch(int pid, BridgeRunContext context, ManualResetEventSlim done)
    {
        Func<int, bool> alive = context.ProcessAlive ?? IsProcessAlive;
        return new Timer(_ =>
        {
            if (!alive(pid))
            {
                SetQuietly(done);
            }
        }, null, TimeSpan.Zero, context.ParentPollInterval);
    }

    /// <summary>Whether process <paramref name="pid"/> exists and has not exited.</summary>
    /// <param name="pid">Process id.</param>
    /// <returns>True while it runs.</returns>
    public static bool IsProcessAlive(int pid)
    {
        try
        {
            using Process p = Process.GetProcessById(pid);
            return !p.HasExited;
        }
        catch (Exception e) when (e is ArgumentException or InvalidOperationException or System.ComponentModel.Win32Exception)
        {
            return false;
        }
    }

    private static void WaitForEof(TextReader stdin, ManualResetEventSlim done)
    {
        try
        {
            while (stdin.Read() >= 0)
            {
                // Content is ignored; only end of file matters.
            }
        }
        catch (Exception e) when (e is IOException or ObjectDisposedException)
        {
            // A broken stdin counts as closed.
        }
        SetQuietly(done);
    }

    private static void SetQuietly(ManualResetEventSlim done)
    {
        try
        {
            done.Set();
        }
        catch (ObjectDisposedException)
        {
            // Already shut down by another signal.
        }
    }
}
