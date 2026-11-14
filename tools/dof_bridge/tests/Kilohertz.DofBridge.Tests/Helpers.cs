using System.Buffers.Binary;

namespace Kilohertz.DofBridge.Tests;

/// <summary>Builds telemetry v1 datagrams for tests.</summary>
internal static class Dg
{
    public static byte[] Make(ulong tick, bool full, params (TelemetryKind Kind, ushort Id, float Value)[] entries)
    {
        var d = new byte[TelemetryParser.HeaderLength + (entries.Length * TelemetryParser.EntryLength)];
        "PNTL"u8.CopyTo(d);
        d[4] = 1;
        d[5] = (byte)(full ? 1 : 0);
        BinaryPrimitives.WriteUInt16LittleEndian(d.AsSpan(6), (ushort)entries.Length);
        BinaryPrimitives.WriteUInt64LittleEndian(d.AsSpan(8), tick);
        for (int i = 0; i < entries.Length; i++)
        {
            int o = TelemetryParser.HeaderLength + (i * TelemetryParser.EntryLength);
            d[o] = (byte)entries[i].Kind;
            BinaryPrimitives.WriteUInt16LittleEndian(d.AsSpan(o + 2), entries[i].Id);
            BinaryPrimitives.WriteSingleLittleEndian(d.AsSpan(o + 4), entries[i].Value);
        }
        return d;
    }

    public static (TelemetryKind, ushort, float) Sw(ushort id, float v) => (TelemetryKind.Switch, id, v);
    public static (TelemetryKind, ushort, float) Coil(ushort id, float v) => (TelemetryKind.Coil, id, v);
    public static (TelemetryKind, ushort, float) Lamp(ushort id, float v) => (TelemetryKind.Lamp, id, v);
    public static (TelemetryKind, ushort, float) Gi(ushort id, float v) => (TelemetryKind.Gi, id, v);
}

/// <summary>Records sink calls, tagged with the record index the test sets.</summary>
internal sealed class RecordingSink : IDofSink
{
    private readonly object _lock = new();
    public List<string> Calls { get; } = new(4096);
    public int CurrentRecord { get; set; } = -1;
    public string? InitArgs { get; private set; }
    public bool Finished { get; private set; }
    public bool TagWithRecord { get; set; }

    public void Init(string romName, string tableName) => InitArgs = $"{romName}|{tableName}";

    public void ReceiveData(char tableElementType, int number, int value)
    {
        lock (_lock)
        {
            Calls.Add(TagWithRecord ? $"{CurrentRecord} {tableElementType} {number} {value}" : $"{tableElementType} {number} {value}");
        }
    }

    public void Finish() => Finished = true;

    public string[] Snapshot()
    {
        lock (_lock)
        {
            return [.. Calls];
        }
    }
}

internal static class Fixtures
{
    public static string Dir => Path.Combine(AppContext.BaseDirectory, "fixtures");

    public static ResolvedDofMap Map() => ResolvedDofMap.Load(Path.Combine(Dir, "map.json"));

    public static List<byte[]> Records(string name)
    {
        byte[] all = File.ReadAllBytes(Path.Combine(Dir, name + ".datagrams.bin"));
        var list = new List<byte[]>();
        int o = 0;
        while (o < all.Length)
        {
            int len = (int)BinaryPrimitives.ReadUInt32LittleEndian(all.AsSpan(o));
            list.Add(all.AsSpan(o + 4, len).ToArray());
            o += 4 + len;
        }
        return list;
    }

    public static string[] Expected(string name) =>
        File.ReadAllLines(Path.Combine(Dir, name + ".expected.txt"))
            .Where(l => l.Length > 0 && !l.StartsWith('#'))
            .ToArray();

    /// <summary>Expected lines without the record index.</summary>
    public static string[] ExpectedUpdates(string name) =>
        Expected(name).Select(l => l[(l.IndexOf(' ', StringComparison.Ordinal) + 1)..]).ToArray();
}

internal static class TempDir
{
    public static string Create()
    {
        string d = Path.Combine(Path.GetTempPath(), "khz-dof-tests-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(d);
        return d;
    }
}
