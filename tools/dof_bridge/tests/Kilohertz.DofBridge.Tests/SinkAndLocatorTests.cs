using Xunit;

namespace Kilohertz.DofBridge.Tests;

public class SinkAndLocatorTests
{
    private static string FakeDll => Path.Combine(AppContext.BaseDirectory, "DirectOutput.dll");

    [Fact]
    public void ReflectionSinkDrivesPinball()
    {
        ReflectionDofSink sink = ReflectionDofSink.Load(FakeDll, "global.xml");
        lock (DirectOutput.Pinball.Log)
        {
            DirectOutput.Pinball.Log.Clear();
        }
        sink.Init("rom1", "Table One");
        sink.ReceiveData('S', 3, 1);
        sink.ReceiveData('L', 40, 255);
        sink.Finish();
        string[] log;
        lock (DirectOutput.Pinball.Log)
        {
            log = [.. DirectOutput.Pinball.Log];
        }
        Assert.Equal(["Setup(global.xml|Table One|rom1)", "Init", "S 3 1", "L 40 255", "Finish"], log);
    }

    [Fact]
    public void ReflectionSinkRejectsAssemblyWithoutPinball()
    {
        string core = typeof(BridgeCore).Assembly.Location;
        Assert.Throws<InvalidOperationException>(() => ReflectionDofSink.Load(core, null));
    }

    /// <summary>Review N6 (D25): the untrusted table name reaches DOF as a bare file name.</summary>
    [Theory]
    [InlineData("Apex Circuit", "Apex Circuit")]
    [InlineData("../../Windows/System32/x", "_.._Windows_System32_x")]
    [InlineData("..", "table")]
    [InlineData(" . ", "table")]
    [InlineData("", "table")]
    [InlineData(null, "table")]
    [InlineData("C:\\evil\\name.vpx", "C__evil_name.vpx")]
    [InlineData("a\u0000b\nc<>:\"|?*", "a_b_c_______")]
    [InlineData("...dots...", "dots")]
    [InlineData("\ud83c\udfb0 slot", "__ slot")]
    public void TableNameIsSanitizedForDof(string? input, string expected) =>
        Assert.Equal(expected, ReflectionDofSink.SanitizeTableFileName(input));

    [Fact]
    public void TableNameIsCappedAt64Chars() =>
        Assert.Equal(new string('x', 64), ReflectionDofSink.SanitizeTableFileName(new string('x', 200)));

    [Fact]
    public void ReflectionSinkPassesSanitizedTableName()
    {
        ReflectionDofSink sink = ReflectionDofSink.Load(FakeDll, null);
        lock (DirectOutput.Pinball.Log)
        {
            DirectOutput.Pinball.Log.Clear();
        }
        sink.Init("rom", "../x");
        lock (DirectOutput.Pinball.Log)
        {
            Assert.Equal("Setup(|_x|rom)", DirectOutput.Pinball.Log[0]);
        }
    }

    /// <summary>Review N5: wrongly shaped DirectOutput.Pinball types fail as InvalidOperationException.</summary>
    [Theory]
    [InlineData("noctor")]
    [InlineData("badreceive")]
    [InlineData("throwingctor")]
    public void WronglyShapedPinballFailsClosed(string shape)
    {
        System.Reflection.Assembly asm = FakePinball.Build(shape);
        InvalidOperationException e = Assert.Throws<InvalidOperationException>(() => ReflectionDofSink.FromAssembly(asm, null));
        Assert.Contains("not a usable DirectOutput assembly", e.Message);
    }

    [Fact]
    public void MissingMethodFailsClosed()
    {
        System.Reflection.Assembly asm = FakePinball.Build("nofinish");
        Assert.Throws<InvalidOperationException>(() => ReflectionDofSink.FromAssembly(asm, null));
    }

    [Fact]
    public void LogSinkFormat()
    {
        var w = new StringWriter();
        var s = new LogDofSink(w);
        s.Init("r", "t");
        s.ReceiveData('G', 2, 128);
        s.Finish();
        Assert.Equal($"# init rom=r table=t{Environment.NewLine}G 2 128{Environment.NewLine}# finish{Environment.NewLine}", w.ToString());
        var n = new NullDofSink();
        n.Init("r", "t");
        n.ReceiveData('S', 1, 1);
        n.Finish();
    }

    [Fact]
    public void LocatorSearchOrder()
    {
        string root = TempDir.Create();
        string arch = Environment.Is64BitProcess ? "x64" : "x86";
        string a = Path.Combine(root, "a");
        string b = Path.Combine(root, "b");
        Directory.CreateDirectory(Path.Combine(a, arch));
        Directory.CreateDirectory(b);
        File.WriteAllText(Path.Combine(a, "DirectOutput.dll"), "");
        File.WriteAllText(Path.Combine(a, arch, "DirectOutput.dll"), "");
        File.WriteAllText(Path.Combine(b, "DirectOutput.dll"), "");
        string? NoEnv(string _) => null;

        // Directory: architecture subfolder first.
        Assert.Equal(Path.Combine(a, arch, "DirectOutput.dll"), DofLocator.FindDirectOutputDll(a, NoEnv, []));
        // Explicit file.
        Assert.Equal(Path.Combine(b, "DirectOutput.dll"), DofLocator.FindDirectOutputDll(Path.Combine(b, "DirectOutput.dll"), NoEnv, []));
        // Explicit path that does not exist: no fallback.
        Assert.Null(DofLocator.FindDirectOutputDll(Path.Combine(root, "missing"), NoEnv, [b]));
        // Environment variable before the search list.
        Assert.Equal(Path.Combine(b, "DirectOutput.dll"),
            DofLocator.FindDirectOutputDll(null, n => n == DofLocator.EnvironmentVariable ? b : null, [a]));
        // Search list in order, skipping misses.
        Assert.Equal(Path.Combine(b, "DirectOutput.dll"),
            DofLocator.FindDirectOutputDll(null, n => Path.Combine(root, "nothing"), [Path.Combine(root, "x"), root, b]));
        Assert.Null(DofLocator.FindDirectOutputDll(null, NoEnv, []));
        _ = DofLocator.DefaultSearchDirectories();
        _ = DofLocator.FindDirectOutputDll(Path.Combine(root, "missing"));
        Directory.Delete(root, true);
    }
}
