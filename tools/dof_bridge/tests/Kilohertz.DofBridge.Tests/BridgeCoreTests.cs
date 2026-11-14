using Xunit;
using static Kilohertz.DofBridge.Tests.Dg;

namespace Kilohertz.DofBridge.Tests;

public class BridgeCoreTests
{
    private static ResolvedDofMap SmallMap() => new("rom", "Table",
    [
        new DofMapEntry(DofSourceKind.Coil, 1, "flip", 'S', 1),
        new DofMapEntry(DofSourceKind.Switch, 21, "sling", 'W', 21),
        new DofMapEntry(DofSourceKind.Lamp, 40, "insert", 'L', 40),
        new DofMapEntry(DofSourceKind.Gi, 0, "gi", 'G', 1),
        new DofMapEntry(DofSourceKind.Coil, 12, "bumper", 'S', 10),
        new DofMapEntry(DofSourceKind.Coil, 12, "bumper", 'E', 200),
    ]);

    private static (BridgeCore, RecordingSink) Core()
    {
        var sink = new RecordingSink();
        return (new BridgeCore(SmallMap(), sink), sink);
    }

    private static byte[] FullState(ulong tick, float flip = 0, float sling = 0, float lamp = 0, float gi = 0, float bumper = 0) =>
        Make(tick, true, Sw(21, sling), Coil(1, flip), Coil(12, bumper), Lamp(40, lamp), Gi(0, gi));

    [Theory]
    [InlineData(DofSourceKind.Switch, 0f, 0)]
    [InlineData(DofSourceKind.Switch, 1f, 1)]
    [InlineData(DofSourceKind.Coil, 0.49f, 0)]
    [InlineData(DofSourceKind.Coil, 0.5f, 1)]
    [InlineData(DofSourceKind.Coil, float.NaN, 0)]
    [InlineData(DofSourceKind.Lamp, 0f, 0)]
    [InlineData(DofSourceKind.Lamp, 1f, 255)]
    [InlineData(DofSourceKind.Lamp, 0.5f, 128)]
    [InlineData(DofSourceKind.Lamp, 0.001f, 0)]
    [InlineData(DofSourceKind.Lamp, 0.002f, 1)]
    [InlineData(DofSourceKind.Gi, 2f, 255)]
    [InlineData(DofSourceKind.Gi, -1f, 0)]
    [InlineData(DofSourceKind.Gi, float.NaN, 0)]
    [InlineData(DofSourceKind.Gi, float.PositiveInfinity, 255)]
    public void ConvertValue(DofSourceKind k, float v, int expected) =>
        Assert.Equal(expected, BridgeCore.ConvertValue(k, v));

    [Fact]
    public void LampConversionIsMonotonicAndCoversAllSteps()
    {
        int prev = 0;
        var seen = new HashSet<int>();
        for (int i = 0; i <= 10000; i++)
        {
            int v = BridgeCore.ConvertValue(DofSourceKind.Lamp, i / 10000f);
            Assert.True(v >= prev);
            prev = v;
            seen.Add(v);
        }
        Assert.Equal(256, seen.Count);
    }

    [Fact]
    public void FirstFullStateForwardsEveryMapping()
    {
        var (core, sink) = Core();
        Assert.Equal(ProcessResult.Applied, core.Process(FullState(0), 0));
        Assert.Equal(["W 21 0", "S 1 0", "S 10 0", "E 200 0", "L 40 0", "G 1 0"], sink.Snapshot());
        Assert.Equal(1, core.Stats.Resyncs);
    }

    [Fact]
    public void PeriodicFullStateResendsOnlyChanges()
    {
        var (core, sink) = Core();
        core.Process(FullState(0), 0);
        sink.Calls.Clear();
        core.Process(FullState(1000, lamp: 0.5f), 0);
        Assert.Equal(["L 40 128"], sink.Snapshot());
        core.Process(FullState(2000, lamp: 0.5f), 0);
        Assert.Equal(["L 40 128"], sink.Snapshot());
        Assert.Equal(1, core.Stats.Resyncs);
    }

    [Fact]
    public void DeltasForwardOnlyChangedDofValues()
    {
        var (core, sink) = Core();
        core.Process(FullState(0), 0);
        sink.Calls.Clear();
        core.Process(Make(1, false, Lamp(40, 0.5f)), 0);
        core.Process(Make(2, false, Lamp(40, 0.501f)), 0); // still 128
        core.Process(Make(3, false, Lamp(40, 0.51f)), 0);  // 130
        core.Process(Make(4, false, Coil(1, 1), Coil(1, 1)), 0);
        Assert.Equal(["L 40 128", "L 40 130", "S 1 1"], sink.Snapshot());
    }

    [Fact]
    public void OneSourceDrivesSeveralOutputsInMapOrder()
    {
        var (core, sink) = Core();
        core.Process(FullState(0), 0);
        sink.Calls.Clear();
        core.Process(Make(5, false, Coil(12, 1)), 0);
        Assert.Equal(["S 10 1", "E 200 1"], sink.Snapshot());
    }

    [Fact]
    public void UnmappedMagnetsAndImpulsesAreIgnored()
    {
        var (core, sink) = Core();
        core.Process(FullState(0), 0);
        sink.Calls.Clear();
        core.Process(Make(1, false, Coil(2, 1), Lamp(41, 1), (TelemetryKind.Magnet, 0, 1f), (TelemetryKind.HapticImpulse, 12, 0.8f), Gi(5, 1)), 0);
        Assert.Empty(sink.Snapshot());
        Assert.Equal(2, core.Stats.Accepted);
    }

    [Fact]
    public void StaleDeltaBelowLastFullStateIsIgnored()
    {
        var (core, sink) = Core();
        core.Process(FullState(0), 0);
        core.Process(FullState(1000), 0);
        sink.Calls.Clear();
        Assert.Equal(ProcessResult.Stale, core.Process(Make(999, false, Coil(1, 1)), 0));
        Assert.Empty(sink.Snapshot());
        Assert.Equal(1, core.Stats.Stale);
        // Same tick as the full state is fine.
        Assert.Equal(ProcessResult.Applied, core.Process(Make(1000, false, Coil(1, 1)), 0));
        Assert.Equal(["S 1 1"], sink.Snapshot());
    }

    [Fact]
    public void ReorderedDeltaForAnElementIsIgnored()
    {
        var (core, sink) = Core();
        core.Process(FullState(0), 0);
        sink.Calls.Clear();
        core.Process(Make(11, false, Coil(1, 0), Lamp(40, 1)), 0); // newer arrives first
        core.Process(Make(10, false, Coil(1, 1), Sw(21, 1)), 0);   // older: coil ignored, switch applies
        Assert.Equal(["L 40 255", "W 21 1"], sink.Snapshot());
    }

    [Fact]
    public void ReorderedFullStateDoesNotOverrideNewerDelta()
    {
        var (core, sink) = Core();
        core.Process(FullState(0), 0);
        sink.Calls.Clear();
        core.Process(Make(1001, false, Coil(1, 1)), 0);
        core.Process(FullState(1000), 0); // periodic, coil=0 at 1000 is older than 1001
        Assert.Equal(["S 1 1"], sink.Snapshot());
    }

    [Fact]
    public void DuplicateDatagramForwardsNothingNew()
    {
        var (core, sink) = Core();
        byte[] full = FullState(0, flip: 1);
        core.Process(full, 0);
        core.Process(full, 0);
        byte[] d = Make(3, false, Lamp(40, 1));
        core.Process(d, 0);
        core.Process(d, 0);
        Assert.Equal(7, sink.Snapshot().Length);
    }

    [Fact]
    public void SplitResyncFullStateSendsEachMappingOnce()
    {
        var (core, sink) = Core();
        // A full state split over two datagrams (same tick).
        core.Process(Make(0, true, Sw(21, 0), Coil(1, 0)), 0);
        core.Process(Make(0, true, Coil(12, 0), Lamp(40, 0), Gi(0, 0)), 0);
        Assert.Equal(6, sink.Snapshot().Length);
        Assert.Equal(1, core.Stats.Resyncs);
    }

    [Fact]
    public void SenderRestartResyncsEverything()
    {
        var (core, sink) = Core();
        core.Process(FullState(0, gi: 1), 0);
        core.Process(FullState(5000, gi: 1), 0);
        sink.Calls.Clear();
        // New session: full state with a lower tick, same values: all sent again.
        Assert.Equal(ProcessResult.Applied, core.Process(FullState(0, gi: 1), 0));
        Assert.Equal(["W 21 0", "S 1 0", "S 10 0", "E 200 0", "L 40 0", "G 1 255"], sink.Snapshot());
        Assert.Equal(2, core.Stats.Resyncs);
        // Low-tick deltas of the new session now apply.
        sink.Calls.Clear();
        core.Process(Make(1, false, Coil(1, 1)), 0);
        Assert.Equal(["S 1 1"], sink.Snapshot());
    }

    [Fact]
    public void DeltasBeforeFirstFullStateAreForwarded()
    {
        var (core, sink) = Core();
        core.Process(Make(400, false, Coil(1, 1)), 0);
        core.Process(Make(300, false, Lamp(40, 1)), 0); // no full state yet: nothing is stale
        core.Process(Make(401, false, Coil(1, 1)), 0);   // unchanged
        Assert.Equal(["S 1 1", "L 40 255"], sink.Snapshot());
        sink.Calls.Clear();
        core.Process(FullState(1000, flip: 1, lamp: 1), 0); // first full state: everything once
        Assert.Equal(6, sink.Snapshot().Length);
    }

    [Fact]
    public void RejectedDatagramHasNoEffect()
    {
        var (core, sink) = Core();
        byte[] bad = FullState(0);
        bad[0] = 0;
        Assert.Equal(ProcessResult.Rejected, core.Process(bad, 0));
        Assert.Empty(sink.Snapshot());
        Assert.Equal(1, core.Stats.Rejected);
        Assert.Equal(0, core.Stats.Resyncs);
    }

    [Fact]
    public void LatencyIsMeasuredFromReceiveStampToSinkCall()
    {
        long now = 1000;
        var sink = new RecordingSink();
        var core = new BridgeCore(SmallMap(), sink, () => now);
        core.Process(FullState(0), 900);
        Assert.Equal(100, core.Stats.MaxForwardLatencyTicks);
        now = 5000;
        core.Process(Make(1, false, Coil(1, 1)), 4990);
        Assert.Equal(100, core.Stats.MaxForwardLatencyTicks);
        core.Process(Make(2, false, Coil(1, 0)), 4000);
        Assert.Equal(1000, core.Stats.MaxForwardLatencyTicks);
        // No forward, no latency sample.
        core.Process(Make(3, false, Coil(1, 0)), 0);
        Assert.Equal(1000, core.Stats.MaxForwardLatencyTicks);
        Assert.Equal(TimeSpan.FromSeconds(1000.0 / System.Diagnostics.Stopwatch.Frequency), core.Stats.MaxForwardLatency);
    }

    [Fact]
    public void BadMapEntryIdThrows()
    {
        var map = new ResolvedDofMap("r", "t", [new DofMapEntry(DofSourceKind.Lamp, 256, "x", 'L', 1)]);
        Assert.Throws<ArgumentOutOfRangeException>(() => new BridgeCore(map, new NullDofSink()));
        Assert.Throws<ArgumentNullException>(() => new BridgeCore(null!, new NullDofSink()));
        Assert.Throws<ArgumentNullException>(() => new BridgeCore(map, null!));
    }

    [Fact]
    public void WarmupRuns() => BridgeCore.Warmup();
}
