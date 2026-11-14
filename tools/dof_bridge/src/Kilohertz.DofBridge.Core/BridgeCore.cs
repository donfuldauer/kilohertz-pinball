using System.Diagnostics;

namespace Kilohertz.DofBridge;

/// <summary>What <see cref="BridgeCore.Process"/> did with a datagram.</summary>
public enum ProcessResult
{
    /// <summary>Valid and applied (it may have forwarded nothing).</summary>
    Applied,
    /// <summary>Failed a protocol §3 receiver check; ignored.</summary>
    Rejected,
    /// <summary>A non-full-state datagram older than the last full state; ignored.</summary>
    Stale,
}

/// <summary>A snapshot of <see cref="BridgeCore"/> counters.</summary>
public readonly record struct BridgeStats
{
    /// <summary>Datagrams applied.</summary>
    public long Accepted { get; init; }
    /// <summary>Datagrams rejected by the parser.</summary>
    public long Rejected { get; init; }
    /// <summary>Datagrams ignored as stale.</summary>
    public long Stale { get; init; }
    /// <summary>Sink calls made.</summary>
    public long Forwarded { get; init; }
    /// <summary>Resyncs started (first full state, sender restarts).</summary>
    public long Resyncs { get; init; }
    /// <summary>Largest receive-to-sink-call latency, in <see cref="Stopwatch"/> ticks (p100).</summary>
    public long MaxForwardLatencyTicks { get; init; }

    /// <summary><see cref="MaxForwardLatencyTicks"/> as a <see cref="TimeSpan"/>.</summary>
    public TimeSpan MaxForwardLatency => TimeSpan.FromSeconds((double)MaxForwardLatencyTicks / Stopwatch.Frequency);
}

/// <summary>
/// The bridge's translation core: fed telemetry datagrams with their receive timestamps, it keeps a
/// mirror of every mapped element and calls the <see cref="IDofSink"/> for each change. Single-threaded
/// and allocation-free per datagram. Rules: DECISIONS.md "3.5 — DOF bridge public API".
/// </summary>
public sealed class BridgeCore
{
    private const int KindCount = 4;
    private const int IdsPerKind = 256;
    private const int SlotCount = KindCount * IdsPerKind;

    private readonly IDofSink _sink;
    private readonly Func<long> _clock;

    // Mapping entries grouped by slot (kind * 256 + id): _slotStart[s].._slotStart[s] + _slotCount[s] in _order.
    private readonly int[] _slotStart = new int[SlotCount];
    private readonly int[] _slotCount = new int[SlotCount];
    private readonly int[] _order;
    private readonly char[] _type;
    private readonly int[] _number;
    private readonly int[] _lastSent;
    private readonly bool[] _resyncSent;
    private readonly ulong[] _elemTick = new ulong[SlotCount];
    private readonly bool[] _elemHasTick = new bool[SlotCount];

    private bool _hasFull;
    private ulong _lastFullTick;
    private ulong _maxTick;
    private bool _resyncActive;
    private ulong _resyncTick;

    private long _accepted;
    private long _rejected;
    private long _stale;
    private long _forwarded;
    private long _resyncs;
    private long _maxLatency;

    /// <summary>
    /// A full state whose tick is at least this many ticks (100 ms) below the newest tick applied
    /// since the last resync means the sender restarted (a new session whose full-state tick is not
    /// below the last one, e.g. both at tick 0), not reordering: it starts a resync like a full state
    /// with a lower tick (TELEMETRY_PROTOCOL §6: full-state entries are authoritative).
    /// </summary>
    public const ulong RestartGapTicks = 100;

    /// <summary>Creates a core.</summary>
    /// <param name="map">Resolved mapping.</param>
    /// <param name="sink">Update destination (already initialised).</param>
    /// <param name="clock">Monotonic clock in <see cref="Stopwatch"/> ticks; default <see cref="Stopwatch.GetTimestamp"/>.</param>
    public BridgeCore(ResolvedDofMap map, IDofSink sink, Func<long>? clock = null)
    {
        ArgumentNullException.ThrowIfNull(map);
        ArgumentNullException.ThrowIfNull(sink);
        _sink = sink;
        _clock = clock ?? Stopwatch.GetTimestamp;
        int n = map.Entries.Count;
        _order = new int[n];
        _type = new char[n];
        _number = new int[n];
        _lastSent = new int[n];
        _resyncSent = new bool[n];
        Array.Fill(_lastSent, -1);
        foreach (DofMapEntry e in map.Entries)
        {
            _slotCount[Slot(e)]++;
        }
        int start = 0;
        for (int s = 0; s < SlotCount; s++)
        {
            _slotStart[s] = start;
            start += _slotCount[s];
        }
        var fill = new int[SlotCount];
        for (int i = 0; i < n; i++)
        {
            DofMapEntry e = map.Entries[i];
            int s = Slot(e);
            _order[_slotStart[s] + fill[s]] = i;
            fill[s]++;
            _type[i] = e.Type;
            _number[i] = e.Number;
        }
    }

    private static int Slot(DofMapEntry e)
    {
        if (e.Id < 0 || e.Id >= IdsPerKind)
        {
            throw new ArgumentOutOfRangeException(nameof(e), $"DOF map entry id {e.Id} is outside 0..255");
        }
        return ((int)e.Source * IdsPerKind) + e.Id;
    }

    /// <summary>Counters so far. Safe to read from another thread once that thread has observed
    /// a later write by the processing thread (e.g. <see cref="UdpBridgeHost.Received"/>).</summary>
    public BridgeStats Stats => new()
    {
        Accepted = Volatile.Read(ref _accepted),
        Rejected = Volatile.Read(ref _rejected),
        Stale = Volatile.Read(ref _stale),
        Forwarded = Volatile.Read(ref _forwarded),
        Resyncs = Volatile.Read(ref _resyncs),
        MaxForwardLatencyTicks = Volatile.Read(ref _maxLatency),
    };

    /// <summary>The DOF value for a telemetry value: switches/coils 0/1 (≥ 0.5 is 1), lamps/GI 0..255.</summary>
    /// <param name="source">Element kind.</param>
    /// <param name="value">Telemetry value.</param>
    /// <returns>The value sent to DOF.</returns>
    public static int ConvertValue(DofSourceKind source, float value)
    {
        if (source is DofSourceKind.Switch or DofSourceKind.Coil)
        {
            return value >= 0.5f ? 1 : 0;
        }
        if (float.IsNaN(value))
        {
            return 0;
        }
        double d = Math.Clamp((double)value, 0.0, 1.0);
        return (int)Math.Floor((d * 255.0) + 0.5);
    }

    /// <summary>Applies one datagram and forwards every resulting change to the sink.</summary>
    /// <param name="datagram">Raw datagram bytes.</param>
    /// <param name="receiveTimestamp">When it arrived, in the clock's units (latency reference).</param>
    /// <returns>What happened to the datagram.</returns>
    public ProcessResult Process(ReadOnlySpan<byte> datagram, long receiveTimestamp)
    {
        if (TelemetryParser.TryParse(datagram, out TelemetryHeader h) != ParseStatus.Ok)
        {
            Volatile.Write(ref _rejected, _rejected + 1);
            return ProcessResult.Rejected;
        }
        ulong tick = h.Tick;
        bool full = h.IsFullState;
        if (full)
        {
            if (!_hasFull || tick < _lastFullTick || (_maxTick > tick && _maxTick - tick >= RestartGapTicks))
            {
                _hasFull = true;
                _lastFullTick = tick;
                _maxTick = tick;
                _resyncActive = true;
                _resyncTick = tick;
                Array.Clear(_elemHasTick);
                Array.Clear(_resyncSent);
                Volatile.Write(ref _resyncs, _resyncs + 1);
            }
            else if (tick > _lastFullTick)
            {
                _lastFullTick = tick;
            }
        }
        else if (_hasFull && tick < _lastFullTick)
        {
            Volatile.Write(ref _stale, _stale + 1);
            return ProcessResult.Stale;
        }

        if (tick > _maxTick)
        {
            _maxTick = tick;
        }
        bool forcing = full && _resyncActive && tick == _resyncTick;
        for (int i = 0; i < h.EntryCount; i++)
        {
            TelemetryEntry e = TelemetryParser.ReadEntry(datagram, i);
            if (e.Kind > TelemetryKind.Gi || e.Id >= IdsPerKind)
            {
                continue;
            }
            int slot = ((int)e.Kind * IdsPerKind) + e.Id;
            int count = _slotCount[slot];
            if (count == 0)
            {
                continue;
            }
            if (_elemHasTick[slot] && tick < _elemTick[slot])
            {
                continue;
            }
            _elemHasTick[slot] = true;
            _elemTick[slot] = tick;
            int value = ConvertValue((DofSourceKind)e.Kind, e.Value);
            int start = _slotStart[slot];
            for (int k = start; k < start + count; k++)
            {
                int m = _order[k];
                if ((forcing && !_resyncSent[m]) || _lastSent[m] != value)
                {
                    long latency = _clock() - receiveTimestamp;
                    if (latency > _maxLatency)
                    {
                        Volatile.Write(ref _maxLatency, latency);
                    }
                    _sink.ReceiveData(_type[m], _number[m], value);
                    _lastSent[m] = value;
                    Volatile.Write(ref _forwarded, _forwarded + 1);
                }
                if (forcing)
                {
                    _resyncSent[m] = true;
                }
            }
        }
        Volatile.Write(ref _accepted, _accepted + 1);
        return ProcessResult.Applied;
    }

    /// <summary>
    /// Runs a synthetic full state through a scratch core with a <see cref="NullDofSink"/> so the
    /// JIT has compiled the datagram path before the first real packet arrives.
    /// </summary>
    public static void Warmup()
    {
        var map = new ResolvedDofMap("warmup", "warmup", [new DofMapEntry(DofSourceKind.Lamp, 0, "warmup", 'L', 0)]);
        var core = new BridgeCore(map, new NullDofSink());
        Span<byte> d = stackalloc byte[TelemetryParser.HeaderLength + TelemetryParser.EntryLength];
        "PNTL"u8.CopyTo(d);
        d[4] = TelemetryParser.Version;
        for (int pass = 0; pass < 4; pass++)
        {
            d[5] = (byte)(pass == 0 ? TelemetryParser.FlagFullState : 0);
            d[6] = 1;
            d[8] = (byte)pass;
            d[16] = (byte)TelemetryKind.Lamp;
            System.Buffers.Binary.BinaryPrimitives.WriteSingleLittleEndian(d.Slice(20, 4), pass * 0.25f);
            core.Process(d, Stopwatch.GetTimestamp());
        }
    }
}
