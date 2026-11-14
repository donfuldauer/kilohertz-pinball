using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using Xunit;

namespace Kilohertz.DofBridge.Tests;

/// <summary>
/// Verification criterion, sequence half: recorded telemetry (generated with pin_audio::telemetry
/// by crates/pin_app/tests/dof_fixtures.rs) replayed into the bridge with a mock DOF sink gives the
/// exact update sequence (single shot). The 2 ms latency half is <see cref="LatencyTests"/>.
/// </summary>
public class FixtureReplayTests
{

    [Theory]
    [InlineData("clean")]
    [InlineData("faulty")]
    public void ExactSequenceThroughCore(string name)
    {
        BridgeCore.Warmup(); // as UdpBridgeHost.Start does: JIT before the first stamped datagram
        var sink = new RecordingSink { TagWithRecord = true };
        sink.ReceiveData('S', 0, 0);
        sink.Calls.Clear();
        var core = new BridgeCore(Fixtures.Map(), sink);
        List<byte[]> records = Fixtures.Records(name);
        for (int i = 0; i < records.Count; i++)
        {
            sink.CurrentRecord = i;
            core.Process(records[i], Stopwatch.GetTimestamp());
        }
        string[] expected = Fixtures.Expected(name);
        Assert.True(expected.Length > 100);
        Assert.Equal(expected, sink.Snapshot());
        Assert.Equal(expected.Length, core.Stats.Forwarded);
    }

    [Fact]
    public void FaultyFixtureExercisesEveryReceiverPath()
    {
        var core = new BridgeCore(Fixtures.Map(), new NullDofSink());
        foreach (byte[] r in Fixtures.Records("faulty"))
        {
            core.Process(r, 0);
        }
        BridgeStats s = core.Stats;
        Assert.Equal(49, s.Rejected); // 7 garbage kinds at 7 places
        Assert.True(s.Stale >= 1, "the late delta after the restarted session's full state is stale");
        Assert.Equal(2, s.Resyncs); // joined mid-session, then the sender restarted
    }

    [Theory]
    [InlineData("clean")]
    [InlineData("faulty")]
    public void ExactSequenceOverUdp(string name)
    {
        var sink = new RecordingSink();
        sink.ReceiveData('S', 0, 0); // JIT the sink before measuring
        sink.Calls.Clear();
        var core = new BridgeCore(Fixtures.Map(), sink);
        using var host = new UdpBridgeHost(core, new IPEndPoint(IPAddress.Loopback, 0));
        host.Start();
        using var tx = new Socket(AddressFamily.InterNetwork, SocketType.Dgram, ProtocolType.Udp);
        tx.Connect(host.LocalEndPoint);
        List<byte[]> records = Fixtures.Records(name);
        long sent = 0;
        foreach (byte[] r in records)
        {
            tx.Send(r);
            sent++;
            // One datagram in flight at a time keeps the replay lossless; no sleeping.
            SpinWait.SpinUntil(() => host.Received >= sent, TimeSpan.FromSeconds(10));
            Assert.Equal(sent, host.Received);
        }
        host.Stop();
        Assert.Equal(Fixtures.ExpectedUpdates(name), sink.Snapshot());
        BridgeStats s = core.Stats;
        Assert.Equal(records.Count, s.Accepted + s.Rejected + s.Stale);
    }

    [Fact]
    public void HostStopsCleanlyAndIgnoresOversizedDatagrams()
    {
        var sink = new RecordingSink();
        var core = new BridgeCore(Fixtures.Map(), sink);
        var host = new UdpBridgeHost(core, new IPEndPoint(IPAddress.Loopback, 0));
        host.Start();
        host.Start(); // idempotent
        using var tx = new Socket(AddressFamily.InterNetwork, SocketType.Dgram, ProtocolType.Udp);
        tx.Connect(host.LocalEndPoint);
        tx.Send(new byte[4000]);
        tx.Send(Dg.Make(0, true, Dg.Coil(1, 1)));
        SpinWait.SpinUntil(() => core.Stats.Forwarded >= 1, TimeSpan.FromSeconds(10));
        Assert.Contains("S 1 1", sink.Snapshot());
        host.Dispose();
        // The oversized datagram is received and rejected on every OS (Windows reports it as
        // WSAEMSGSIZE, Linux and macOS truncate it): it must not be dropped uncounted.
        Assert.Equal(1, core.Stats.Rejected);
        Assert.Equal(2, host.Received);
    }

    [Fact]
    public void BindConflictThrows()
    {
        using var taken = new Socket(AddressFamily.InterNetwork, SocketType.Dgram, ProtocolType.Udp);
        taken.Bind(new IPEndPoint(IPAddress.Loopback, 0));
        var core = new BridgeCore(Fixtures.Map(), new NullDofSink());
        Assert.Throws<SocketException>(() => new UdpBridgeHost(core, (IPEndPoint)taken.LocalEndPoint!));
    }
}
