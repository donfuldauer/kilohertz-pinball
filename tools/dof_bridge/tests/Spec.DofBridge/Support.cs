// Subphase 3.5 Independent Tester: shared helpers. Everything here is built from
// docs/TELEMETRY_PROTOCOL.md and the published bridge contract, not from the bridge sources.
using System.Buffers.Binary;
using System.Globalization;
using System.Text;
using Kilohertz.DofBridge;

namespace Spec.DofBridge;

/// <summary>One recorded sink call.</summary>
public readonly record struct Update(char Type, int Number, int Value)
{
    public override string ToString() => $"{Type} {Number} {Value}";
}

/// <summary>Mock DOF sink: records every call, thread-safe (the UDP host calls it from its receive thread).</summary>
public sealed class RecordingSink : IDofSink
{
    private readonly object _gate = new();
    private readonly List<Update> _updates = new();
    private readonly List<string> _calls = new();

    public void Init(string romName, string tableName)
    {
        lock (_gate)
        {
            _calls.Add($"Init({romName},{tableName})");
        }
    }

    public void ReceiveData(char tableElementType, int number, int value)
    {
        lock (_gate)
        {
            _updates.Add(new Update(tableElementType, number, value));
            _calls.Add("ReceiveData");
        }
    }

    public void Finish()
    {
        lock (_gate)
        {
            _calls.Add("Finish");
        }
    }

    public List<Update> Updates
    {
        get
        {
            lock (_gate)
            {
                return new List<Update>(_updates);
            }
        }
    }

    public List<string> Calls
    {
        get
        {
            lock (_gate)
            {
                return new List<string>(_calls);
            }
        }
    }

    public int Count
    {
        get
        {
            lock (_gate)
            {
                return _updates.Count;
            }
        }
    }

    /// <summary>Returns the updates recorded since the previous call.</summary>
    public List<Update> Drain()
    {
        lock (_gate)
        {
            var copy = new List<Update>(_updates);
            _updates.Clear();
            return copy;
        }
    }
}

/// <summary>Independent encoder for telemetry protocol v1 datagrams.</summary>
public static class Wire
{
    public const byte Switch = 0, Coil = 1, Lamp = 2, Gi = 3, Magnet = 4, Impulse = 5;
    public const int Switches = 256, Coils = 256, Lamps = 256, GiStrings = 32, Magnets = 32;

    public readonly record struct Entry(byte Kind, ushort Id, float Value, byte Reserved = 0);

    public static byte[] Datagram(ulong tick, bool full, params Entry[] entries) =>
        Raw(tick, (byte)(full ? 1 : 0), 1, (ushort)entries.Length, entries);

    public static byte[] Delta(ulong tick, params Entry[] entries) => Datagram(tick, false, entries);

    /// <summary>Encodes without any validity check (for hostile datagrams).</summary>
    public static byte[] Raw(ulong tick, byte flags, byte version, ushort declaredCount, IReadOnlyList<Entry> entries)
    {
        var d = new byte[16 + 8 * entries.Count];
        d[0] = (byte)'P';
        d[1] = (byte)'N';
        d[2] = (byte)'T';
        d[3] = (byte)'L';
        d[4] = version;
        d[5] = flags;
        BinaryPrimitives.WriteUInt16LittleEndian(d.AsSpan(6), declaredCount);
        BinaryPrimitives.WriteUInt64LittleEndian(d.AsSpan(8), tick);
        for (int i = 0; i < entries.Count; i++)
        {
            var e = entries[i];
            var s = d.AsSpan(16 + 8 * i);
            s[0] = e.Kind;
            s[1] = e.Reserved;
            BinaryPrimitives.WriteUInt16LittleEndian(s[2..], e.Id);
            BinaryPrimitives.WriteSingleLittleEndian(s[4..], e.Value);
        }
        return d;
    }

    public static Entry Sw(int id, bool closed) => new(Switch, (ushort)id, closed ? 1f : 0f);
    public static Entry Co(int id, bool on) => new(Coil, (ushort)id, on ? 1f : 0f);
    public static Entry La(int id, float v) => new(Lamp, (ushort)id, v);
    public static Entry G(int id, float v) => new(Gi, (ushort)id, v);
    public static Entry Mag(int id, float v) => new(Magnet, (ushort)id, v);
    public static Entry Imp(int id, float v) => new(Impulse, (ushort)id, v);
}

/// <summary>The cabinet state a sender mirrors; produces protocol-conformant full states (832 entries, 5 datagrams).</summary>
public sealed class CabinetState
{
    public readonly float[] Switches = new float[Wire.Switches];
    public readonly float[] Coils = new float[Wire.Coils];
    public readonly float[] Lamps = new float[Wire.Lamps];
    public readonly float[] Gi = new float[Wire.GiStrings];
    public readonly float[] Magnets = new float[Wire.Magnets];

    public List<Wire.Entry> AllEntries()
    {
        var all = new List<Wire.Entry>(832);
        for (int i = 0; i < Switches.Length; i++) all.Add(new(Wire.Switch, (ushort)i, Switches[i]));
        for (int i = 0; i < Coils.Length; i++) all.Add(new(Wire.Coil, (ushort)i, Coils[i]));
        for (int i = 0; i < Lamps.Length; i++) all.Add(new(Wire.Lamp, (ushort)i, Lamps[i]));
        for (int i = 0; i < Gi.Length; i++) all.Add(new(Wire.Gi, (ushort)i, Gi[i]));
        for (int i = 0; i < Magnets.Length; i++) all.Add(new(Wire.Magnet, (ushort)i, Magnets[i]));
        return all;
    }

    /// <summary>The 5 datagrams of a full state at <paramref name="tick"/> (flags bit 0 set on each).</summary>
    public List<byte[]> FullState(ulong tick)
    {
        var all = AllEntries();
        var result = new List<byte[]>();
        for (int at = 0; at < all.Count; at += 180)
        {
            var chunk = all.GetRange(at, Math.Min(180, all.Count - at)).ToArray();
            result.Add(Wire.Datagram(tick, true, chunk));
        }
        if (result.Count != 5) throw new InvalidOperationException("a full state is 5 datagrams");
        return result;
    }
}

/// <summary>Builds resolved-mapping JSON in the published format.</summary>
public sealed class MapBuilder
{
    private readonly List<string> _entries = new();
    public string Rom = "specrom";
    public string Table = "Spec Table";

    public MapBuilder Add(string source, int id, char type, int number, string? name = null)
    {
        name ??= $"{source}_{id}";
        _entries.Add(string.Create(CultureInfo.InvariantCulture,
            $"{{ \"source\": \"{source}\", \"id\": {id}, \"name\": \"{name}\", \"type\": \"{type}\", \"number\": {number} }}"));
        return this;
    }

    public string Json()
    {
        var sb = new StringBuilder();
        sb.Append("{ \"format\": \"kilohertz-dof-map\", \"version\": 1, \"rom_name\": \"").Append(Rom)
            .Append("\", \"table_name\": \"").Append(Table).Append("\", \"entries\": [");
        sb.AppendJoin(", ", _entries);
        sb.Append("] }");
        return sb.ToString();
    }

    public ResolvedDofMap Build() => ResolvedDofMap.Parse(Json());

    /// <summary>The mapping of the recorded session fixture and most scenario tests.</summary>
    public static MapBuilder Standard() => new MapBuilder()
        .Add("coil", 3, 'S', 1)
        .Add("coil", 7, 'S', 12)
        .Add("switch", 10, 'W', 20)
        .Add("switch", 200, 'W', 21)
        .Add("lamp", 5, 'L', 30)
        .Add("lamp", 250, 'L', 31)
        .Add("gi", 2, 'G', 1)
        .Add("gi", 31, 'G', 2);
}

/// <summary>Checked-in fixtures (generated by crates/pin_app/tests/spec_3_5_fixtures.rs).</summary>
public static class Fixtures
{
    public static string Dir => Path.Combine(AppContext.BaseDirectory, "fixtures");

    /// <summary>`[u16 LE length][datagram]` records.</summary>
    public static List<byte[]> Recording(string name)
    {
        var bytes = File.ReadAllBytes(Path.Combine(Dir, name));
        var list = new List<byte[]>();
        int at = 0;
        while (at < bytes.Length)
        {
            int len = BinaryPrimitives.ReadUInt16LittleEndian(bytes.AsSpan(at));
            at += 2;
            list.Add(bytes.AsSpan(at, len).ToArray());
            at += len;
        }
        return list;
    }

    /// <summary>`<type> <number> <value>` lines.</summary>
    public static List<Update> Expected(string name) =>
        File.ReadAllLines(Path.Combine(Dir, name))
            .Where(l => l.Length > 0)
            .Select(l => l.Split(' '))
            .Select(p => new Update(p[0][0], int.Parse(p[1], CultureInfo.InvariantCulture), int.Parse(p[2], CultureInfo.InvariantCulture)))
            .ToList();

    /// <summary>`<telemetry kind> <id> <type> <number>` lines → resolved map.</summary>
    public static ResolvedDofMap Mapping(string name)
    {
        var b = new MapBuilder();
        string[] sources = { "switch", "coil", "lamp", "gi" };
        foreach (var line in File.ReadAllLines(Path.Combine(Dir, name)).Where(l => l.Length > 0))
        {
            var p = line.Split(' ');
            b.Add(sources[int.Parse(p[0], CultureInfo.InvariantCulture)], int.Parse(p[1], CultureInfo.InvariantCulture), p[2][0], int.Parse(p[3], CultureInfo.InvariantCulture));
        }
        return b.Build();
    }
}
