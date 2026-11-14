using System.Globalization;
using System.Net;

namespace Kilohertz.DofBridge;

/// <summary>Which sink the bridge drives.</summary>
public enum SinkKind
{
    /// <summary>The user's DirectOutput Framework (default).</summary>
    Dof,
    /// <summary>Text log of updates.</summary>
    Log,
    /// <summary>Discard.</summary>
    Null,
}

/// <summary>Bad command line.</summary>
public sealed class BridgeUsageException : Exception
{
    /// <summary>Creates the exception.</summary>
    /// <param name="message">What is wrong.</param>
    public BridgeUsageException(string message) : base(message) { }

    /// <summary>Creates the exception.</summary>
    public BridgeUsageException() : base("bad command line") { }

    /// <summary>Creates the exception with an inner cause.</summary>
    /// <param name="message">What is wrong.</param>
    /// <param name="inner">Cause.</param>
    public BridgeUsageException(string message, Exception inner) : base(message, inner) { }
}

/// <summary>Parsed command line (DECISIONS.md "3.5 — DOF bridge public API").</summary>
public sealed class BridgeOptions
{
    /// <summary>Default telemetry port (TELEMETRY_PROTOCOL.md §1).</summary>
    public const int DefaultPort = 35120;

    /// <summary>Usage text.</summary>
    public const string Usage =
        "KilohertzDofBridge --map <file> [--port <u16>] [--bind <loopback ip>] [--rom <name>] [--table <name>]\n" +
        "                   [--dof-path <dir|DirectOutput.dll>] [--dof-config <file>] [--sink dof|log|null]\n" +
        "                   [--log <file>] [--parent-pid <pid>] [--exit-on-stdin-eof] [--help]\n" +
        "Forwards Kilohertz Pinball UDP telemetry to the DirectOutput Framework (unverified on hardware).";

    /// <summary>Resolved-mapping file.</summary>
    public string MapFile { get; private set; } = string.Empty;
    /// <summary>UDP port.</summary>
    public int Port { get; private set; } = DefaultPort;
    /// <summary>Local bind address (<see cref="BridgeApp.Run"/> accepts loopback only).</summary>
    public IPAddress Bind { get; private set; } = IPAddress.Loopback;
    /// <summary>ROM name override.</summary>
    public string? RomName { get; private set; }
    /// <summary>Table name override.</summary>
    public string? TableName { get; private set; }
    /// <summary>DOF install directory or DirectOutput.dll.</summary>
    public string? DofPath { get; private set; }
    /// <summary>DOF global config file.</summary>
    public string? DofConfig { get; private set; }
    /// <summary>Sink kind.</summary>
    public SinkKind Sink { get; private set; } = SinkKind.Dof;
    /// <summary>Log file for <see cref="SinkKind.Log"/> (stdout when null).</summary>
    public string? LogFile { get; private set; }
    /// <summary>Exit when this process exits.</summary>
    public int? ParentPid { get; private set; }
    /// <summary>Exit cleanly when stdin reaches end of file.</summary>
    public bool ExitOnStdinEof { get; private set; }
    /// <summary><c>--help</c> was given.</summary>
    public bool Help { get; private set; }

    /// <summary>Parses arguments.</summary>
    /// <param name="args">Command-line arguments.</param>
    /// <returns>The options.</returns>
    /// <exception cref="BridgeUsageException">Unknown flag, missing or bad value, or no <c>--map</c>.</exception>
    public static BridgeOptions Parse(IReadOnlyList<string> args)
    {
        ArgumentNullException.ThrowIfNull(args);
        var o = new BridgeOptions();
        for (int i = 0; i < args.Count; i++)
        {
            string a = args[i];
            switch (a)
            {
                case "--help":
                case "-h":
                    o.Help = true;
                    break;
                case "--exit-on-stdin-eof":
                    o.ExitOnStdinEof = true;
                    break;
                case "--map":
                    o.MapFile = Value(args, ref i);
                    break;
                case "--port":
                    o.Port = Int(args, ref i, 0, ushort.MaxValue);
                    break;
                case "--bind":
                    string b = Value(args, ref i);
                    o.Bind = IPAddress.TryParse(b, out IPAddress? ip) ? ip : throw new BridgeUsageException($"--bind: '{b}' is not an IP address");
                    break;
                case "--rom":
                    o.RomName = Value(args, ref i);
                    break;
                case "--table":
                    o.TableName = Value(args, ref i);
                    break;
                case "--dof-path":
                    o.DofPath = Value(args, ref i);
                    break;
                case "--dof-config":
                    o.DofConfig = Value(args, ref i);
                    break;
                case "--log":
                    o.LogFile = Value(args, ref i);
                    break;
                case "--parent-pid":
                    o.ParentPid = Int(args, ref i, 1, int.MaxValue);
                    break;
                case "--sink":
                    string s = Value(args, ref i);
                    o.Sink = s switch
                    {
                        "dof" => SinkKind.Dof,
                        "log" => SinkKind.Log,
                        "null" => SinkKind.Null,
                        _ => throw new BridgeUsageException($"--sink: '{s}' is not dof, log or null"),
                    };
                    break;
                default:
                    throw new BridgeUsageException($"unknown argument '{a}'");
            }
        }
        if (!o.Help && o.MapFile.Length == 0)
        {
            throw new BridgeUsageException("--map is required");
        }
        return o;
    }

    private static string Value(IReadOnlyList<string> args, ref int i)
    {
        if (i + 1 >= args.Count)
        {
            throw new BridgeUsageException($"{args[i]} needs a value");
        }
        i++;
        return args[i];
    }

    private static int Int(IReadOnlyList<string> args, ref int i, int min, int max)
    {
        string flag = args[i];
        string v = Value(args, ref i);
        if (!int.TryParse(v, NumberStyles.None, CultureInfo.InvariantCulture, out int n) || n < min || n > max)
        {
            throw new BridgeUsageException($"{flag}: '{v}' is not an integer in {min}..{max}");
        }
        return n;
    }
}
