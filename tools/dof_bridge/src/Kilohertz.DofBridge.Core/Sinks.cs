using System.Reflection;

namespace Kilohertz.DofBridge;

/// <summary>
/// Where the bridge sends table-element updates. The real implementation hosts the user's
/// DirectOutput Framework (<see cref="ReflectionDofSink"/>); tests use a mock.
/// All calls come from one thread (the bridge's receive thread).
/// </summary>
public interface IDofSink
{
    /// <summary>Starts the sink for a table (DOF: <c>Pinball.Setup</c> + <c>Init</c>).</summary>
    /// <param name="romName">ROM name DOF configs are keyed by.</param>
    /// <param name="tableName">Table display name.</param>
    void Init(string romName, string tableName);

    /// <summary>One table-element update (DOF: <c>Pinball.ReceiveData</c>).</summary>
    /// <param name="tableElementType">S, W, L, G or E.</param>
    /// <param name="number">Element number.</param>
    /// <param name="value">0/1 for switches and coils, 0..255 for lamps and GI.</param>
    void ReceiveData(char tableElementType, int number, int value);

    /// <summary>Stops the sink (DOF: <c>Pinball.Finish</c>).</summary>
    void Finish();
}

/// <summary>Discards every update.</summary>
public sealed class NullDofSink : IDofSink
{
    /// <inheritdoc />
    public void Init(string romName, string tableName) { }

    /// <inheritdoc />
    public void ReceiveData(char tableElementType, int number, int value) { }

    /// <inheritdoc />
    public void Finish() { }
}

/// <summary>Writes each update as a <c>"&lt;type&gt; &lt;number&gt; &lt;value&gt;"</c> line (for <c>--sink log</c>).</summary>
public sealed class LogDofSink : IDofSink
{
    private readonly TextWriter _out;

    /// <summary>Creates a sink writing to <paramref name="output"/>.</summary>
    /// <param name="output">Destination; flushed after every line.</param>
    public LogDofSink(TextWriter output)
    {
        _out = output;
    }

    /// <inheritdoc />
    public void Init(string romName, string tableName)
    {
        _out.WriteLine($"# init rom={romName} table={tableName}");
        _out.Flush();
    }

    /// <inheritdoc />
    public void ReceiveData(char tableElementType, int number, int value)
    {
        _out.Write(tableElementType);
        _out.Write(' ');
        _out.Write(number);
        _out.Write(' ');
        _out.WriteLine(value);
        _out.Flush();
    }

    /// <inheritdoc />
    public void Finish()
    {
        _out.WriteLine("# finish");
        _out.Flush();
    }
}

/// <summary>
/// Hosts the user's installed DirectOutput Framework by reflection: <c>DirectOutput.Pinball</c>
/// from <c>DirectOutput.dll</c>, driven through <c>Setup(globalConfig, tableName, romName)</c>,
/// <c>Init()</c>, <c>ReceiveData(char, int, int)</c> and <c>Finish()</c>.
/// <para><b>Unverified on hardware</b> (D33): CI has no DOF install; only a stand-in assembly with
/// the same shape is exercised.</para>
/// </summary>
public sealed class ReflectionDofSink : IDofSink
{
    private readonly object _pinball;
    private readonly MethodInfo _setup;
    private readonly MethodInfo _init;
    private readonly MethodInfo _finish;
    private readonly Action<char, int, int> _receive;
    private readonly string _globalConfig;

    private ReflectionDofSink(object pinball, MethodInfo setup, MethodInfo init, MethodInfo finish, Action<char, int, int> receive, string globalConfig)
    {
        _pinball = pinball;
        _setup = setup;
        _init = init;
        _finish = finish;
        _receive = receive;
        _globalConfig = globalConfig;
    }

    /// <summary>Loads <c>DirectOutput.Pinball</c> from <paramref name="directOutputDll"/>.</summary>
    /// <param name="directOutputDll">Path to DirectOutput.dll.</param>
    /// <param name="globalConfig">DOF global config file, or null for DOF's default lookup.</param>
    /// <returns>A sink bound to a new <c>Pinball</c> instance.</returns>
    /// <exception cref="InvalidOperationException">The assembly lacks the expected type or methods, or
    /// they have the wrong shape (missing members, type-load failures, delegate binding errors).</exception>
    /// <exception cref="IOException">The file cannot be read or loaded.</exception>
    /// <exception cref="BadImageFormatException">The file is not a .NET assembly.</exception>
    public static ReflectionDofSink Load(string directOutputDll, string? globalConfig)
    {
        Assembly asm = Assembly.LoadFrom(Path.GetFullPath(directOutputDll));
        return FromAssembly(asm, globalConfig);
    }

    /// <summary>Binds to <c>DirectOutput.Pinball</c> in an already loaded assembly (what
    /// <see cref="Load"/> does after loading the file).</summary>
    /// <param name="assembly">The DirectOutput assembly.</param>
    /// <param name="globalConfig">DOF global config file, or null for DOF's default lookup.</param>
    /// <returns>A sink bound to a new <c>Pinball</c> instance.</returns>
    /// <exception cref="InvalidOperationException">The type or its methods are missing or have the wrong
    /// shape; every reflection failure (MissingMemberException, TypeLoadException,
    /// ReflectionTypeLoadException, ArgumentException from delegate binding, MemberAccessException,
    /// a throwing constructor) is reported as this, so the bridge fails closed with exit code 4.</exception>
    public static ReflectionDofSink FromAssembly(Assembly assembly, string? globalConfig)
    {
        ArgumentNullException.ThrowIfNull(assembly);
        try
        {
            Type type = assembly.GetType("DirectOutput.Pinball", throwOnError: false)
                ?? throw new InvalidOperationException($"{assembly.GetName().Name}: type DirectOutput.Pinball not found");
            MethodInfo setup = Method(type, "Setup", typeof(string), typeof(string), typeof(string));
            MethodInfo init = Method(type, "Init");
            MethodInfo finish = Method(type, "Finish");
            MethodInfo receive = Method(type, "ReceiveData", typeof(char), typeof(int), typeof(int));
            object pinball = Activator.CreateInstance(type)
                ?? throw new InvalidOperationException("DirectOutput.Pinball could not be created");
            var receiveDelegate = receive.CreateDelegate<Action<char, int, int>>(pinball);
            return new ReflectionDofSink(pinball, setup, init, finish, receiveDelegate, globalConfig ?? string.Empty);
        }
        catch (Exception e) when (e is MissingMemberException or TypeLoadException or ReflectionTypeLoadException
                                      or ArgumentException or MemberAccessException or TargetInvocationException)
        {
            throw new InvalidOperationException(
                $"{assembly.GetName().Name}: not a usable DirectOutput assembly ({e.GetType().Name}: {e.Message})", e);
        }
    }

    private static MethodInfo Method(Type type, string name, params Type[] args)
    {
        return type.GetMethod(name, BindingFlags.Public | BindingFlags.Instance, args)
            ?? throw new InvalidOperationException($"DirectOutput.Pinball.{name}({string.Join(", ", args.Select(a => a.Name))}) not found");
    }

    /// <summary>Calls DOF <c>Setup(globalConfig, SanitizeTableFileName(tableName), romName)</c> and
    /// <c>Init()</c>. DOF treats the table argument as a table <i>file name</i> (it looks for
    /// table-specific config next to it), and the name comes from the untrusted <c>[meta].name</c>
    /// (D25), so it is reduced to a bare file name first.</summary>
    /// <param name="romName">ROM name DOF configs key on.</param>
    /// <param name="tableName">Table display name.</param>
    public void Init(string romName, string tableName)
    {
        _setup.Invoke(_pinball, [_globalConfig, SanitizeTableFileName(tableName), romName]);
        _init.Invoke(_pinball, null);
    }

    /// <summary>
    /// Reduces an untrusted table name to a safe bare file name for DOF: control characters,
    /// path separators, drive colons and the other characters invalid in Windows file names
    /// (<c>&lt;&gt;:"/\|?*</c>) become <c>_</c>; leading and trailing dots and spaces are trimmed
    /// (so <c>..</c> cannot remain a path component); at most 64 characters are kept; an empty
    /// result becomes <c>table</c>.
    /// </summary>
    /// <param name="tableName">Untrusted name (e.g. <c>[meta].name</c>).</param>
    /// <returns>A non-empty name with no path syntax.</returns>
    public static string SanitizeTableFileName(string? tableName)
    {
        const int MaxLength = 64;
        var sb = new System.Text.StringBuilder(Math.Min((tableName ?? string.Empty).Length, MaxLength));
        foreach (char c in tableName ?? string.Empty)
        {
            if (sb.Length == MaxLength)
            {
                break;
            }
            bool bad = char.IsControl(c) || char.IsSurrogate(c) || c is '<' or '>' or ':' or '"' or '/' or '\\' or '|' or '?' or '*';
            sb.Append(bad ? '_' : c);
        }
        string name = sb.ToString().Trim(' ', '.');
        return name.Length == 0 ? "table" : name;
    }

    /// <inheritdoc />
    public void ReceiveData(char tableElementType, int number, int value) => _receive(tableElementType, number, value);

    /// <inheritdoc />
    public void Finish() => _finish.Invoke(_pinball, null);
}
