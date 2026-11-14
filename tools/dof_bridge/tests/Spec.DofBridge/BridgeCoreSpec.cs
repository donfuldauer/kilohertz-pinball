// Subphase 3.5 Independent Tester: BridgeCore forwarding rules, from the brief, TELEMETRY_PROTOCOL.md
// (receiver rules, listener guidance) and the published contract.
using System.Diagnostics;
using Kilohertz.DofBridge;
using Xunit;

namespace Spec.DofBridge;

public class BridgeCoreSpec
{
    private sealed class Rig
    {
        public readonly RecordingSink Sink = new();
        public readonly BridgeCore Core;
        public long Now = 1_000;

        public Rig(MapBuilder? map = null)
        {
            Core = new BridgeCore((map ?? MapBuilder.Standard()).Build(), Sink, () => Now);
        }

        public ProcessResult Send(byte[] datagram) => Core.Process(datagram, Now);

        public void Send(IEnumerable<byte[]> datagrams)
        {
            foreach (var d in datagrams) Core.Process(d, Now);
        }

        public List<Update> Take() => Sink.Drain();
    }

    private static Update U(char t, int n, int v) => new(t, n, v);

    /// <summary>The standard map's 8 elements, in full-state entry order, all at zero.</summary>
    private static readonly Update[] AllZero =
    {
        U('W', 20, 0), U('W', 21, 0), U('S', 1, 0), U('S', 12, 0), U('L', 30, 0), U('L', 31, 0), U('G', 1, 0), U('G', 2, 0),
    };

    // ----------------------------------------------------------------------------------------
    // Recorded telemetry → exact update sequence (verification criterion)
    // ----------------------------------------------------------------------------------------

    [Fact]
    public void Recorded_session_produces_the_exact_update_sequence()
    {
        var sink = new RecordingSink();
        long now = 0;
        var core = new BridgeCore(Fixtures.Mapping("session.mapping.txt"), sink, () => now);
        var recording = Fixtures.Recording("session.bin");
        Assert.Equal(25, recording.Count);
        foreach (var d in recording)
        {
            now += 17;
            Assert.Equal(ProcessResult.Applied, core.Process(d, now));
        }
        var expected = Fixtures.Expected("session.expected.txt");
        Assert.Equal(23, expected.Count);
        Assert.Equal(expected, sink.Updates);
        Assert.Equal(expected.Count, core.Stats.Forwarded);
        Assert.Equal(recording.Count, core.Stats.Accepted);
        Assert.Equal(0, core.Stats.Rejected);
        Assert.Equal(0, core.Stats.Stale);
        Assert.Equal(1, core.Stats.Resyncs);
        // BridgeCore forwards table-element updates only; Init/Finish belong to the host.
        Assert.All(sink.Calls, c => Assert.Equal("ReceiveData", c));
    }

    [Fact]
    public void Recorded_session_is_forwarded_within_2_ms_of_arrival()
    {
        var map = Fixtures.Mapping("session.mapping.txt");
        var recording = Fixtures.Recording("session.bin");
        var expected = Fixtures.Expected("session.expected.txt");

        // Warm-up (JIT) on a separate instance, then the measured replay with the bridge's default
        // monotonic clock: receive stamp = Stopwatch.GetTimestamp() at hand-over, as the UDP host does.
        for (int i = 0; i < 20; i++)
        {
            var warm = new BridgeCore(map, new RecordingSink());
            foreach (var d in recording) warm.Process(d, Stopwatch.GetTimestamp());
        }
        var sink = new RecordingSink();
        var core = new BridgeCore(map, sink);
        foreach (var d in recording)
        {
            core.Process(d, Stopwatch.GetTimestamp());
        }
        Assert.Equal(expected, sink.Updates);
        var stats = core.Stats;
        Assert.True(stats.MaxForwardLatencyTicks >= 0);
        Assert.True(stats.MaxForwardLatency <= TimeSpan.FromMilliseconds(2),
            $"p100 receive-to-sink latency {stats.MaxForwardLatency.TotalMilliseconds:F4} ms exceeds 2 ms");
    }

    [Fact]
    public void Latency_is_measured_from_the_receive_stamp_to_the_sink_call_on_the_bridge_clock()
    {
        var sink = new RecordingSink();
        long now = 0;
        var core = new BridgeCore(MapBuilder.Standard().Build(), sink, () => now);
        Assert.Equal(0, core.Stats.MaxForwardLatencyTicks);
        Assert.Equal(TimeSpan.Zero, core.Stats.MaxForwardLatency);

        // Nothing forwarded (unmapped element): the latency statistic does not move.
        now = 5_000;
        core.Process(Wire.Delta(1, Wire.Co(99, true)), 1_000);
        Assert.Equal(0, core.Stats.MaxForwardLatencyTicks);

        now = 10_000;
        core.Process(Wire.Delta(2, Wire.Co(3, true)), 9_400);
        Assert.Equal(600, core.Stats.MaxForwardLatencyTicks);

        // A faster forward does not lower the maximum; a slower one raises it.
        now = 20_000;
        core.Process(Wire.Delta(3, Wire.Co(3, false)), 19_990);
        Assert.Equal(600, core.Stats.MaxForwardLatencyTicks);
        now = 30_000;
        core.Process(Wire.Delta(4, Wire.Co(3, true)), 28_750);
        Assert.Equal(1_250, core.Stats.MaxForwardLatencyTicks);

        var want = TimeSpan.FromSeconds(1_250.0 / Stopwatch.Frequency);
        Assert.InRange((core.Stats.MaxForwardLatency - want).Duration().Ticks, 0, 1);
        Assert.Equal(3, sink.Count);
    }

    // ----------------------------------------------------------------------------------------
    // First full state after start
    // ----------------------------------------------------------------------------------------

    [Fact]
    public void First_full_state_sends_every_mapped_element_even_at_zero()
    {
        var rig = new Rig();
        var state = new CabinetState();
        rig.Send(state.FullState(0));
        Assert.Equal(AllZero, rig.Take());
        Assert.Equal(1, rig.Core.Stats.Resyncs);
        Assert.Equal(8, rig.Core.Stats.Forwarded);
        Assert.Equal(5, rig.Core.Stats.Accepted);
    }

    [Fact]
    public void First_full_state_sends_current_values_in_entry_order()
    {
        var rig = new Rig();
        var state = new CabinetState();
        state.Switches[200] = 1;
        state.Coils[7] = 1;
        state.Lamps[5] = 1;
        state.Lamps[250] = 0.2f;
        state.Gi[31] = 0.6f;
        // Unmapped neighbours and never-mapped kinds must not leak in.
        state.Switches[11] = 1;
        state.Coils[4] = 1;
        state.Lamps[6] = 1;
        state.Gi[3] = 1;
        state.Magnets[2] = 1;
        state.Magnets[3] = 1;
        rig.Send(state.FullState(123_456));
        Assert.Equal(new[]
        {
            U('W', 20, 0), U('W', 21, 1), U('S', 1, 0), U('S', 12, 1), U('L', 30, 255), U('L', 31, 51), U('G', 1, 0), U('G', 2, 153),
        }, rig.Take());
    }

    [Fact]
    public void Bridge_joining_mid_session_gets_everything_from_its_first_full_state()
    {
        var rig = new Rig();
        // Deltas before the first full state are forwarded as changes against "unknown".
        Assert.Equal(ProcessResult.Applied, rig.Send(Wire.Delta(5_000, Wire.Co(3, true))));
        Assert.Equal(new[] { U('S', 1, 1) }, rig.Take());
        Assert.Equal(ProcessResult.Applied, rig.Send(Wire.Delta(5_001, Wire.Co(3, true))));
        Assert.Empty(rig.Take());
        Assert.Equal(0, rig.Core.Stats.Resyncs);

        var state = new CabinetState();
        state.Coils[3] = 1;
        rig.Send(state.FullState(5_500));
        // Everything once, including the element that was already forwarded with the same value.
        var want = (Update[])AllZero.Clone();
        want[2] = U('S', 1, 1);
        Assert.Equal(want, rig.Take());
        Assert.Equal(1, rig.Core.Stats.Resyncs);
    }

    [Fact]
    public void A_new_bridge_instance_starts_from_scratch()
    {
        // "(re)start" of the bridge: a fresh core knows nothing and sends everything again.
        var map = MapBuilder.Standard().Build();
        var state = new CabinetState();
        foreach (var _ in Enumerable.Range(0, 2))
        {
            var sink = new RecordingSink();
            var core = new BridgeCore(map, sink, () => 0);
            foreach (var d in state.FullState(9_000)) core.Process(d, 0);
            Assert.Equal(AllZero, sink.Updates);
        }
    }

    // ----------------------------------------------------------------------------------------
    // Change-only forwarding
    // ----------------------------------------------------------------------------------------

    [Fact]
    public void Periodic_full_states_do_not_resend_unchanged_values()
    {
        var rig = new Rig();
        var state = new CabinetState();
        state.Lamps[5] = 0.6f;
        state.Coils[3] = 1;
        rig.Send(state.FullState(0));
        Assert.Equal(8, rig.Take().Count);
        for (ulong tick = 1000; tick <= 10_000; tick += 1000)
        {
            rig.Send(state.FullState(tick));
        }
        Assert.Empty(rig.Take());
        Assert.Equal(1, rig.Core.Stats.Resyncs);
        Assert.Equal(8, rig.Core.Stats.Forwarded);
        Assert.Equal(55, rig.Core.Stats.Accepted);

        // A full state that carries one real change forwards exactly that change.
        state.Lamps[5] = 0.2f;
        rig.Send(state.FullState(11_000));
        Assert.Equal(new[] { U('L', 30, 51) }, rig.Take());
    }

    [Fact]
    public void Only_changes_of_the_dof_value_are_forwarded()
    {
        var rig = new Rig();
        rig.Send(new CabinetState().FullState(0));
        rig.Take();

        rig.Send(Wire.Delta(1, Wire.La(5, 0.2f)));
        Assert.Equal(new[] { U('L', 30, 51) }, rig.Take());
        // Different float, same 0..255 value: not a DOF change.
        rig.Send(Wire.Delta(2, Wire.La(5, 0.2001f)));
        rig.Send(Wire.Delta(3, Wire.La(5, 0.1999f)));
        Assert.Empty(rig.Take());
        rig.Send(Wire.Delta(4, Wire.La(5, 0.204f)));
        Assert.Equal(new[] { U('L', 30, 52) }, rig.Take());

        // Repeated identical states.
        rig.Send(Wire.Delta(5, Wire.Co(3, true)));
        rig.Send(Wire.Delta(6, Wire.Co(3, true)));
        rig.Send(Wire.Delta(7, Wire.Co(3, true), Wire.Sw(10, false)));
        Assert.Equal(new[] { U('S', 1, 1) }, rig.Take());

        // Back and forth is forwarded every time.
        rig.Send(Wire.Delta(8, Wire.Co(3, false)));
        rig.Send(Wire.Delta(9, Wire.Co(3, true)));
        rig.Send(Wire.Delta(10, Wire.Co(3, false)));
        Assert.Equal(new[] { U('S', 1, 0), U('S', 1, 1), U('S', 1, 0) }, rig.Take());
    }

    [Fact]
    public void Duplicated_datagrams_forward_nothing_new()
    {
        var rig = new Rig();
        var state = new CabinetState();
        var full = state.FullState(0);
        rig.Send(full);
        rig.Send(full);
        Assert.Equal(AllZero, rig.Take());
        Assert.Equal(1, rig.Core.Stats.Resyncs);

        var delta = Wire.Delta(40, Wire.Co(3, true), Wire.La(5, 0.6f));
        rig.Send(delta);
        rig.Send(delta);
        rig.Send(delta);
        Assert.Equal(new[] { U('S', 1, 1), U('L', 30, 153) }, rig.Take());
    }

    [Fact]
    public void Several_changes_in_one_datagram_keep_their_entry_order()
    {
        var rig = new Rig();
        rig.Send(new CabinetState().FullState(0));
        rig.Take();
        rig.Send(Wire.Delta(3, Wire.Sw(10, true), Wire.Sw(200, true), Wire.Co(3, true), Wire.Co(7, true),
            Wire.La(5, 1f), Wire.La(250, 1f), Wire.G(2, 1f), Wire.G(31, 1f), Wire.Mag(2, 1f), Wire.Imp(3, 1f)));
        Assert.Equal(new[]
        {
            U('W', 20, 1), U('W', 21, 1), U('S', 1, 1), U('S', 12, 1), U('L', 30, 255), U('L', 31, 255), U('G', 1, 255), U('G', 2, 255),
        }, rig.Take());
    }

    // ----------------------------------------------------------------------------------------
    // Mapping
    // ----------------------------------------------------------------------------------------

    [Fact]
    public void Unmapped_ids_and_unmapped_kinds_are_ignored()
    {
        var rig = new Rig();
        rig.Send(new CabinetState().FullState(0));
        rig.Take();
        long forwarded = rig.Core.Stats.Forwarded;

        // Ids that are mapped under a *different* kind, neighbours, extremes, magnets and impulses.
        var noise = new List<Wire.Entry>();
        foreach (int id in new[] { 0, 1, 2, 4, 6, 8, 9, 11, 199, 201, 249, 251, 255 })
        {
            noise.Add(Wire.Sw(id, true));
            noise.Add(Wire.Co(id, true));
            noise.Add(Wire.La(id, 1f));
        }
        noise.Add(Wire.Sw(3, true));    // coil 3 is mapped, switch 3 is not
        noise.Add(Wire.Sw(7, true));
        noise.Add(Wire.Sw(5, true));
        noise.Add(Wire.Sw(250, true));
        noise.Add(Wire.Sw(2, true));
        noise.Add(Wire.Sw(31, true));
        noise.Add(Wire.Co(10, true));   // switch 10 is mapped, coil 10 is not
        noise.Add(Wire.Co(200, true));
        noise.Add(Wire.Co(5, true));
        noise.Add(Wire.Co(250, true));
        noise.Add(Wire.La(3, 1f));
        noise.Add(Wire.La(7, 1f));
        noise.Add(Wire.La(10, 1f));
        noise.Add(Wire.La(200, 1f));
        noise.Add(Wire.La(2, 1f));
        noise.Add(Wire.La(31, 1f));
        noise.Add(Wire.G(3, 1f));
        noise.Add(Wire.G(5, 1f));
        noise.Add(Wire.G(7, 1f));
        noise.Add(Wire.G(10, 1f));
        noise.Add(Wire.G(30, 1f));
        for (int id = 0; id < 32; id++) noise.Add(Wire.Mag(id, 1f));
        foreach (int id in new[] { 2, 3, 5, 7, 10, 31, 200, 250, 0x1003, 0x2005, 0x3007, 0xFFFF })
        {
            noise.Add(Wire.Imp(id, 1f));
        }
        // Ids beyond the documented ranges are not an error either.
        noise.Add(new Wire.Entry(Wire.Switch, 0x0103, 1f));
        noise.Add(new Wire.Entry(Wire.Coil, 0x0103, 1f));
        noise.Add(new Wire.Entry(Wire.Coil, 0xFF03, 1f));
        noise.Add(new Wire.Entry(Wire.Lamp, 0x0105, 1f));
        noise.Add(new Wire.Entry(Wire.Gi, 0x0102, 1f));
        noise.Add(new Wire.Entry(Wire.Gi, 34, 1f));
        Assert.Equal(ProcessResult.Applied, rig.Send(Wire.Delta(10, noise.ToArray())));
        Assert.Empty(rig.Take());
        Assert.Equal(forwarded, rig.Core.Stats.Forwarded);

        // The mapped elements still work afterwards.
        rig.Send(Wire.Delta(11, Wire.Co(3, true)));
        Assert.Equal(new[] { U('S', 1, 1) }, rig.Take());
    }

    [Fact]
    public void An_empty_map_forwards_nothing()
    {
        var rig = new Rig(new MapBuilder());
        var state = new CabinetState();
        state.Coils[3] = 1;
        rig.Send(state.FullState(0));
        rig.Send(Wire.Delta(1, Wire.Co(3, false), Wire.La(5, 1f)));
        Assert.Empty(rig.Take());
        Assert.Equal(6, rig.Core.Stats.Accepted);
        Assert.Equal(0, rig.Core.Stats.Forwarded);
    }

    [Fact]
    public void One_element_can_drive_several_dof_outputs_and_any_type()
    {
        var map = new MapBuilder()
            .Add("coil", 3, 'S', 1)
            .Add("coil", 3, 'E', 4001)
            .Add("lamp", 5, 'L', 30)
            .Add("lamp", 5, 'W', 77)        // the DOF type is the table author's choice
            .Add("gi", 2, 'L', 200)
            .Add("switch", 255, 'W', 0)
            .Add("coil", 255, 'S', 65535)
            .Add("gi", 31, 'G', 31);
        var rig = new Rig(map);
        rig.Send(new CabinetState().FullState(0));
        var first = rig.Take();
        Assert.Equal(8, first.Count);
        Assert.Equal(first.ToHashSet(), new[]
        {
            U('S', 1, 0), U('E', 4001, 0), U('L', 30, 0), U('W', 77, 0), U('L', 200, 0), U('W', 0, 0), U('S', 65535, 0), U('G', 31, 0),
        }.ToHashSet());

        rig.Send(Wire.Delta(1, Wire.Co(3, true)));
        Assert.Equal(new[] { U('S', 1, 1), U('E', 4001, 1) }.ToHashSet(), rig.Take().ToHashSet());
        rig.Send(Wire.Delta(2, Wire.La(5, 0.6f)));
        Assert.Equal(new[] { U('L', 30, 153), U('W', 77, 153) }.ToHashSet(), rig.Take().ToHashSet());
        rig.Send(Wire.Delta(3, Wire.Sw(255, true), Wire.Co(255, true), Wire.G(2, 1f), Wire.G(31, 0.2f)));
        Assert.Equal(new[] { U('W', 0, 1), U('S', 65535, 1), U('L', 200, 255), U('G', 31, 51) }, rig.Take());
    }

    // ----------------------------------------------------------------------------------------
    // Value scaling
    // ----------------------------------------------------------------------------------------

    [Theory]
    [InlineData(0f, 0)]
    [InlineData(1f, 255)]
    [InlineData(0.5f, 128)]          // 127.5 rounds half up
    [InlineData(0.2f, 51)]
    [InlineData(0.4f, 102)]
    [InlineData(0.6f, 153)]
    [InlineData(0.8f, 204)]
    [InlineData(0.001f, 0)]
    [InlineData(0.00197f, 1)]        // just above 0.5/255
    [InlineData(0.999f, 255)]
    [InlineData(0.997f, 254)]
    [InlineData(1.5f, 255)]
    [InlineData(1e30f, 255)]
    [InlineData(float.PositiveInfinity, 255)]
    [InlineData(-0.25f, 0)]
    [InlineData(-1e30f, 0)]
    [InlineData(float.NegativeInfinity, 0)]
    [InlineData(float.NaN, 0)]
    public void Lamp_and_gi_levels_scale_to_0_255(float level, int want)
    {
        Assert.Equal(want, BridgeCore.ConvertValue(DofSourceKind.Lamp, level));
        Assert.Equal(want, BridgeCore.ConvertValue(DofSourceKind.Gi, level));
    }

    [Theory]
    [InlineData(0f, 0)]
    [InlineData(1f, 1)]
    [InlineData(float.NaN, 0)]
    [InlineData(0.49f, 0)]
    [InlineData(0.5f, 1)]
    [InlineData(2f, 1)]
    [InlineData(-1f, 0)]
    [InlineData(float.PositiveInfinity, 1)]
    [InlineData(float.NegativeInfinity, 0)]
    public void Switches_and_coils_are_0_or_1(float value, int want)
    {
        Assert.Equal(want, BridgeCore.ConvertValue(DofSourceKind.Switch, value));
        Assert.Equal(want, BridgeCore.ConvertValue(DofSourceKind.Coil, value));
    }

    [Fact]
    public void Level_scaling_is_monotone_nearest_and_covers_every_step()
    {
        var seen = new bool[256];
        int previous = 0;
        const int steps = 200_000;
        for (int i = 0; i <= steps; i++)
        {
            float v = (float)i / steps;
            int got = BridgeCore.ConvertValue(DofSourceKind.Lamp, v);
            Assert.InRange(got, 0, 255);
            Assert.True(got >= previous, $"not monotone at {v}");
            Assert.True(Math.Abs(got - (double)v * 255.0) <= 0.5 + 1e-9, $"{v} → {got} is not the nearest step");
            Assert.Equal(got, BridgeCore.ConvertValue(DofSourceKind.Gi, v));
            previous = got;
            seen[got] = true;
        }
        Assert.All(seen, Assert.True);

        // k/255 as the sender would compute it lands exactly on step k.
        for (int k = 0; k <= 255; k++)
        {
            Assert.Equal(k, BridgeCore.ConvertValue(DofSourceKind.Lamp, k / 255f));
        }
    }

    [Fact]
    public void Arbitrary_float_bit_patterns_convert_without_throwing()
    {
        var rng = new Random(3535);
        foreach (DofSourceKind kind in Enum.GetValues<DofSourceKind>())
        {
            for (int i = 0; i < 100_000; i++)
            {
                float v = BitConverter.Int32BitsToSingle(rng.Next(int.MinValue, int.MaxValue));
                int got = BridgeCore.ConvertValue(kind, v);
                Assert.InRange(got, 0, kind is DofSourceKind.Lamp or DofSourceKind.Gi ? 255 : 1);
            }
        }
    }

    [Fact]
    public void Scaling_is_applied_on_the_way_to_the_sink()
    {
        var rig = new Rig();
        rig.Send(new CabinetState().FullState(0));
        rig.Take();
        var levels = new (float Level, int Dof)[] { (1f, 255), (0.5f, 128), (0.25f, 64), (0.75f, 191), (2f, 255), (0f, 0), (float.NaN, 0), (0.1f, 26) };
        ulong tick = 1;
        int last = 0;
        foreach (var (level, dof) in levels)
        {
            rig.Send(Wire.Delta(tick++, Wire.La(5, level), Wire.G(2, level)));
            var got = rig.Take();
            if (dof == last)
            {
                Assert.Empty(got);
            }
            else
            {
                Assert.Equal(new[] { U('L', 30, dof), U('G', 1, dof) }, got);
            }
            last = dof;
        }
    }

    // ----------------------------------------------------------------------------------------
    // Rejected datagrams (protocol section 3)
    // ----------------------------------------------------------------------------------------

    [Fact]
    public void Rejected_datagrams_have_no_effect()
    {
        var rig = new Rig();
        rig.Send(new CabinetState().FullState(0));
        rig.Take();

        var good = Wire.Delta(10, Wire.Co(3, true), Wire.La(5, 1f));
        var hostile = new List<byte[]>
        {
            Array.Empty<byte>(),
            good.AsSpan(0, 15).ToArray(),
            good.AsSpan(0, good.Length - 1).ToArray(),
            good.Concat(new byte[] { 0 }).ToArray(),
            good.Concat(new byte[8]).ToArray(),
            Wire.Raw(10, 0, 2, 1, new[] { Wire.Co(3, true) }),
            Wire.Raw(10, 0, 0, 1, new[] { Wire.Co(3, true) }),
            Wire.Raw(10, 0, 1, 2, new[] { Wire.Co(3, true) }),
            Wire.Raw(10, 0, 1, 181, Enumerable.Range(0, 181).Select(_ => Wire.Co(3, true)).ToArray()),
            // A bad kind in the *last* entry rejects the whole datagram, including the valid first entries.
            Wire.Datagram(10, false, Wire.Co(3, true), Wire.La(5, 1f), new Wire.Entry(6, 0, 0f)),
            Wire.Datagram(10, true, Wire.Co(3, true), new Wire.Entry(200, 3, 1f)),
        };
        var badMagic = (byte[])good.Clone();
        badMagic[3] = (byte)'X';
        hostile.Add(badMagic);

        foreach (var d in hostile)
        {
            Assert.Equal(ProcessResult.Rejected, rig.Send(d));
        }
        Assert.Empty(rig.Take());
        Assert.Equal(hostile.Count, rig.Core.Stats.Rejected);
        Assert.Equal(5, rig.Core.Stats.Accepted);
        Assert.Equal(1, rig.Core.Stats.Resyncs);

        // The stream is undisturbed: the good datagram still forwards its changes.
        Assert.Equal(ProcessResult.Applied, rig.Send(good));
        Assert.Equal(new[] { U('S', 1, 1), U('L', 30, 255) }, rig.Take());
    }

    [Fact]
    public void A_rejected_full_state_does_not_start_a_resync()
    {
        var rig = new Rig();
        var d = new CabinetState().FullState(0)[0];
        d[4] = 9; // version
        Assert.Equal(ProcessResult.Rejected, rig.Send(d));
        Assert.Equal(0, rig.Core.Stats.Resyncs);
        Assert.Equal(0, rig.Core.Stats.Accepted);
        Assert.Empty(rig.Take());
    }

    // ----------------------------------------------------------------------------------------
    // Packet loss
    // ----------------------------------------------------------------------------------------

    [Fact]
    public void A_lost_delta_is_repaired_by_the_next_full_state()
    {
        var rig = new Rig();
        var state = new CabinetState();
        rig.Send(state.FullState(0));
        rig.Take();

        // Lost on the wire: coil 3 on, lamp 250 to 0.8.
        state.Coils[3] = 1;
        state.Lamps[250] = 0.8f;
        // Delivered: switch 10 closes.
        state.Switches[10] = 1;
        rig.Send(Wire.Delta(501, Wire.Sw(10, true)));
        Assert.Equal(new[] { U('W', 20, 1) }, rig.Take());

        rig.Send(state.FullState(1000));
        Assert.Equal(new[] { U('S', 1, 1), U('L', 31, 204) }, rig.Take());
        Assert.Equal(1, rig.Core.Stats.Resyncs);
    }

    [Fact]
    public void A_lost_part_of_the_first_full_state_is_completed_by_the_next_one()
    {
        var rig = new Rig();
        var state = new CabinetState();
        state.Coils[7] = 1;
        var full = state.FullState(0);
        // Datagram 2 (entries 360..539: coils 104..255 and lamps 0..27) is lost: lamp 5 is unknown.
        rig.Send(full.Where((_, i) => i != 2));
        Assert.Equal(new[]
        {
            U('W', 20, 0), U('W', 21, 0), U('S', 1, 0), U('S', 12, 1), U('L', 31, 0), U('G', 1, 0), U('G', 2, 0),
        }, rig.Take());

        // The next periodic full state supplies the missing element (even though it is 0 and
        // "unchanged" at the sender) and nothing else.
        rig.Send(state.FullState(1000));
        Assert.Equal(new[] { U('L', 30, 0) }, rig.Take());
        rig.Send(state.FullState(2000));
        Assert.Empty(rig.Take());
    }

    [Fact]
    public void Losing_a_whole_periodic_full_state_is_harmless()
    {
        var rig = new Rig();
        var state = new CabinetState();
        rig.Send(state.FullState(0));
        rig.Take();
        // The full state at 1000 never arrives; deltas after it are still applied.
        state.Coils[3] = 1;
        Assert.Equal(ProcessResult.Applied, rig.Send(Wire.Delta(1500, Wire.Co(3, true))));
        Assert.Equal(new[] { U('S', 1, 1) }, rig.Take());
        rig.Send(state.FullState(2000));
        Assert.Empty(rig.Take());
    }

    // ----------------------------------------------------------------------------------------
    // Reordering
    // ----------------------------------------------------------------------------------------

    [Fact]
    public void A_delta_older_than_the_last_full_state_is_stale()
    {
        var rig = new Rig();
        var state = new CabinetState();
        rig.Send(state.FullState(0));
        rig.Send(state.FullState(1000));
        rig.Take();

        // Sent at tick 990 (coil on), overtaken by the full state of tick 1000 (coil off again).
        Assert.Equal(ProcessResult.Stale, rig.Send(Wire.Delta(990, Wire.Co(3, true), Wire.Sw(10, true))));
        Assert.Equal(ProcessResult.Stale, rig.Send(Wire.Delta(999, Wire.La(5, 1f))));
        Assert.Empty(rig.Take());
        Assert.Equal(2, rig.Core.Stats.Stale);
        Assert.Equal(10, rig.Core.Stats.Accepted);

        // The tick of the full state itself and later ticks are not stale.
        Assert.Equal(ProcessResult.Applied, rig.Send(Wire.Delta(1000, Wire.Co(7, true))));
        Assert.Equal(ProcessResult.Applied, rig.Send(Wire.Delta(1001, Wire.Co(3, true))));
        Assert.Equal(new[] { U('S', 12, 1), U('S', 1, 1) }, rig.Take());
    }

    [Fact]
    public void A_late_delta_does_not_overwrite_a_newer_value_of_the_same_element()
    {
        var rig = new Rig();
        rig.Send(new CabinetState().FullState(0));
        rig.Take();

        // Sender: tick 40 coil 3 on + switch 10 closed; tick 50 coil 3 off. The network swaps them.
        rig.Send(Wire.Delta(50, Wire.Co(3, false), Wire.La(5, 0.6f)));
        Assert.Equal(new[] { U('L', 30, 153) }, rig.Take());
        rig.Send(Wire.Delta(40, Wire.Co(3, true), Wire.Sw(10, true), Wire.La(5, 0.2f)));
        // Coil 3 and lamp 5 already have tick-50 values; switch 10 has not been touched since tick 0.
        Assert.Equal(new[] { U('W', 20, 1) }, rig.Take());

        // Later ticks apply normally.
        rig.Send(Wire.Delta(60, Wire.Co(3, true)));
        Assert.Equal(new[] { U('S', 1, 1) }, rig.Take());
    }

    [Fact]
    public void A_swapped_pair_of_deltas_ends_on_the_newest_state()
    {
        var rig = new Rig();
        rig.Send(new CabinetState().FullState(0));
        rig.Take();
        // In order this would be on (70) then off (71). Swapped, the element must end "off".
        rig.Send(Wire.Delta(71, Wire.Co(3, false)));
        rig.Send(Wire.Delta(70, Wire.Co(3, true)));
        Assert.Empty(rig.Take());

        // And on (80) then level change (81) swapped ends on the tick-81 value.
        rig.Send(Wire.Delta(81, Wire.G(2, 0.2f)));
        rig.Send(Wire.Delta(80, Wire.G(2, 1f)));
        Assert.Equal(new[] { U('G', 1, 51) }, rig.Take());
    }

    [Fact]
    public void A_full_state_overtaken_by_a_newer_delta_keeps_the_newer_value()
    {
        var rig = new Rig();
        var state = new CabinetState();
        rig.Send(state.FullState(0));
        rig.Take();
        // Delta of tick 1001 (coil on) arrives before the full state of tick 1000 (coil still off).
        rig.Send(Wire.Delta(1001, Wire.Co(3, true)));
        Assert.Equal(new[] { U('S', 1, 1) }, rig.Take());
        rig.Send(state.FullState(1000));
        Assert.Empty(rig.Take());
        Assert.Equal(1, rig.Core.Stats.Resyncs);
    }

    [Fact]
    public void Full_state_datagrams_in_any_order_send_everything_exactly_once()
    {
        var state = new CabinetState();
        state.Switches[200] = 1;
        state.Lamps[250] = 1;
        state.Gi[2] = 0.4f;
        var want = new[]
        {
            U('W', 20, 0), U('W', 21, 1), U('S', 1, 0), U('S', 12, 0), U('L', 30, 0), U('L', 31, 255), U('G', 1, 102), U('G', 2, 0),
        }.ToHashSet();
        var rng = new Random(77);
        for (int round = 0; round < 50; round++)
        {
            var rig = new Rig();
            var full = state.FullState(4242);
            var order = Enumerable.Range(0, 5).OrderBy(_ => rng.Next()).ToArray();
            foreach (int i in order) rig.Send(full[i]);
            var got = rig.Take();
            Assert.Equal(8, got.Count);
            Assert.Equal(want, got.ToHashSet());
            Assert.Equal(1, rig.Core.Stats.Resyncs);
        }
    }

    /// <summary>
    /// Property: whatever the delivery order of the deltas between two full states (with duplicates), the
    /// values last sent to DOF equal the sender's true state once the next full state has arrived, and an
    /// in-order replay forwards exactly the change sequence.
    /// </summary>
    [Fact]
    public void Shuffled_and_lossy_delivery_converges_on_the_sender_state()
    {
        var rng = new Random(2026);
        var mapped = new (byte Kind, int Id, char Type, int Number)[]
        {
            (Wire.Coil, 3, 'S', 1), (Wire.Coil, 7, 'S', 12), (Wire.Switch, 10, 'W', 20), (Wire.Switch, 200, 'W', 21),
            (Wire.Lamp, 5, 'L', 30), (Wire.Lamp, 250, 'L', 31), (Wire.Gi, 2, 'G', 1), (Wire.Gi, 31, 'G', 2),
        };
        for (int round = 0; round < 200; round++)
        {
            var rig = new Rig();
            var state = new CabinetState();
            rig.Send(state.FullState(0));
            var last = new Dictionary<(char, int), int>();
            foreach (var u in rig.Take()) last[(u.Type, u.Number)] = u.Value;

            for (ulong second = 0; second < 3; second++)
            {
                var deltas = new List<byte[]>();
                for (ulong tick = second * 1000 + 1; tick < second * 1000 + 1000; tick += (ulong)rng.Next(1, 120))
                {
                    var entries = new List<Wire.Entry>();
                    foreach (var m in mapped)
                    {
                        if (rng.Next(3) != 0) continue;
                        float v = m.Kind is Wire.Lamp or Wire.Gi ? rng.Next(0, 6) / 5f : rng.Next(2);
                        float[] target = m.Kind switch
                        {
                            Wire.Switch => state.Switches,
                            Wire.Coil => state.Coils,
                            Wire.Lamp => state.Lamps,
                            _ => state.Gi,
                        };
                        if (BitConverter.SingleToInt32Bits(target[m.Id]) == BitConverter.SingleToInt32Bits(v)) continue;
                        target[m.Id] = v;
                        entries.Add(new Wire.Entry(m.Kind, (ushort)m.Id, v));
                    }
                    if (entries.Count > 0)
                    {
                        // Protocol order within a tick: switches, coils, lamps, GI.
                        deltas.Add(Wire.Delta(tick, entries.OrderBy(e => e.Kind).ThenBy(e => e.Id).ToArray()));
                    }
                }
                // Network: drop some, duplicate some, shuffle.
                var delivered = deltas.Where(_ => rng.Next(4) != 0).ToList();
                delivered.AddRange(delivered.Where(_ => rng.Next(5) == 0).ToList());
                foreach (var d in delivered.OrderBy(_ => rng.Next())) rig.Send(d);
                foreach (var d in state.FullState((second + 1) * 1000).OrderBy(_ => rng.Next())) rig.Send(d);

                foreach (var u in rig.Take())
                {
                    // Change-only: never the value that element already had.
                    Assert.NotEqual(last[(u.Type, u.Number)], u.Value);
                    last[(u.Type, u.Number)] = u.Value;
                }
                foreach (var m in mapped)
                {
                    float truth = m.Kind switch
                    {
                        Wire.Switch => state.Switches[m.Id],
                        Wire.Coil => state.Coils[m.Id],
                        Wire.Lamp => state.Lamps[m.Id],
                        _ => state.Gi[m.Id],
                    };
                    int want = m.Kind is Wire.Lamp or Wire.Gi ? (int)Math.Round(truth * 255.0) : (int)truth;
                    Assert.True(want == last[(m.Type, m.Number)],
                        $"round {round}, second {second}: {m.Type}{m.Number} is {last[(m.Type, m.Number)]} at DOF, sender has {want}");
                }
            }
            Assert.Equal(1, rig.Core.Stats.Resyncs);
            Assert.Equal(0, rig.Core.Stats.Rejected);
        }
    }

    // ----------------------------------------------------------------------------------------
    // Sender restart
    // ----------------------------------------------------------------------------------------

    [Fact]
    public void A_full_state_with_a_lower_tick_is_a_new_session_and_sends_everything_again()
    {
        var rig = new Rig();
        var state = new CabinetState();
        state.Coils[3] = 1;
        state.Lamps[5] = 0.6f;
        rig.Send(state.FullState(60_000));
        rig.Send(Wire.Delta(60_010, Wire.Sw(10, true)));
        var before = new[]
        {
            U('W', 20, 0), U('W', 21, 0), U('S', 1, 1), U('S', 12, 0), U('L', 30, 153), U('L', 31, 0), U('G', 1, 0), U('G', 2, 0), U('W', 20, 1),
        };
        Assert.Equal(before, rig.Take());

        // The engine restarts (or loads another table): tick starts at 0 again, with a full state.
        var fresh = new CabinetState();
        fresh.Lamps[5] = 0.6f;      // happens to be the same as before
        fresh.Gi[2] = 1f;
        rig.Send(fresh.FullState(0));
        Assert.Equal(new[]
        {
            U('W', 20, 0), U('W', 21, 0), U('S', 1, 0), U('S', 12, 0), U('L', 30, 153), U('L', 31, 0), U('G', 1, 255), U('G', 2, 0),
        }, rig.Take());
        Assert.Equal(2, rig.Core.Stats.Resyncs);

        // Deltas of the new session (ticks far below the old session's) are applied, not stale.
        Assert.Equal(ProcessResult.Applied, rig.Send(Wire.Delta(3, Wire.Co(3, true))));
        Assert.Equal(ProcessResult.Applied, rig.Send(Wire.Delta(4, Wire.Sw(10, true), Wire.La(5, 0f))));
        Assert.Equal(new[] { U('S', 1, 1), U('W', 20, 1), U('L', 30, 0) }, rig.Take());
        Assert.Equal(0, rig.Core.Stats.Stale);

        // The new session's periodic full state is an ordinary one again.
        fresh.Coils[3] = 1;
        fresh.Switches[10] = 1;
        fresh.Lamps[5] = 0f;
        rig.Send(fresh.FullState(1000));
        Assert.Empty(rig.Take());
        Assert.Equal(2, rig.Core.Stats.Resyncs);
    }

    [Fact]
    public void A_restart_resync_spans_all_datagrams_of_the_new_full_state_once()
    {
        var rig = new Rig();
        var state = new CabinetState();
        rig.Send(state.FullState(5_000));
        rig.Take();
        var full = state.FullState(7);
        // Duplicated and reordered datagrams of the new session's full state.
        rig.Send(new[] { full[4], full[0], full[4], full[2], full[1], full[3], full[0], full[2] });
        var got = rig.Take();
        Assert.Equal(8, got.Count);
        Assert.Equal(AllZero.ToHashSet(), got.ToHashSet());
        Assert.Equal(2, rig.Core.Stats.Resyncs);
    }

    [Fact]
    public void Repeated_restarts_each_resync()
    {
        var rig = new Rig();
        var state = new CabinetState();
        ulong[] sessionStarts = { 9_000, 500, 499, 0 };
        for (int i = 0; i < sessionStarts.Length; i++)
        {
            rig.Send(state.FullState(sessionStarts[i]));
            Assert.Equal(AllZero, rig.Take());
            Assert.Equal(i + 1, rig.Core.Stats.Resyncs);
        }
        // Same tick again is a duplicate, not a restart.
        rig.Send(state.FullState(0));
        Assert.Empty(rig.Take());
        Assert.Equal(sessionStarts.Length, rig.Core.Stats.Resyncs);
    }

    // ----------------------------------------------------------------------------------------
    // Robustness
    // ----------------------------------------------------------------------------------------

    /// <summary>Gate 5 for the whole receive path: random and mutated datagrams never make Process throw.</summary>
    [Fact]
    public void Fuzz_process_never_throws_and_only_emits_mapped_elements()
    {
        var rng = new Random(0x0D0F);
        var sink = new RecordingSink();
        var core = new BridgeCore(MapBuilder.Standard().Build(), sink);
        var allowed = new Dictionary<(char, int), int>
        {
            [('S', 1)] = 1, [('S', 12)] = 1, [('W', 20)] = 1, [('W', 21)] = 1,
            [('L', 30)] = 255, [('L', 31)] = 255, [('G', 1)] = 255, [('G', 2)] = 255,
        };
        var state = new CabinetState();
        var seeds = state.FullState(100);
        seeds.Add(Wire.Delta(101, Wire.Co(3, true), Wire.La(5, 0.5f), Wire.Sw(10, true), Wire.G(2, 0.3f)));
        long processed = 0, rejected = 0;
        var buffer = new byte[1500];
        for (int i = 0; i < 200_000; i++)
        {
            byte[] d;
            if ((i & 1) == 0)
            {
                d = (byte[])seeds[rng.Next(seeds.Count)].Clone();
                int flips = rng.Next(0, 4);
                for (int f = 0; f < flips; f++) d[rng.Next(d.Length)] = (byte)rng.Next(256);
                if (rng.Next(10) == 0) d = d.AsSpan(0, rng.Next(d.Length + 1)).ToArray();
            }
            else
            {
                int len = rng.Next(3) == 0 ? 16 + 8 * rng.Next(0, 182) : rng.Next(0, buffer.Length);
                d = new byte[len];
                rng.NextBytes(d);
                if (len >= 16 && rng.Next(4) != 0)
                {
                    "PNTL"u8.CopyTo(d);
                    d[4] = 1;
                    d[6] = (byte)((len - 16) / 8);
                    d[7] = (byte)(((len - 16) / 8) >> 8);
                    for (int e = 16; e < len; e += 8) d[e] = (byte)rng.Next(0, 6);
                }
            }
            ProcessResult result;
            try
            {
                result = core.Process(d, Stopwatch.GetTimestamp());
            }
            catch (Exception ex)
            {
                Assert.Fail($"Process threw {ex.GetType().Name} ({ex.Message}) on iteration {i}: {Convert.ToHexString(d.AsSpan(0, Math.Min(d.Length, 64)))} (len {d.Length})");
                return;
            }
            processed++;
            bool valid = ParserSpec.ReferenceAccepts(d);
            Assert.Equal(!valid, result == ProcessResult.Rejected);
            if (!valid) rejected++;
        }
        Assert.Equal(processed, core.Stats.Accepted + core.Stats.Rejected + core.Stats.Stale);
        Assert.Equal(rejected, core.Stats.Rejected);
        Assert.True(core.Stats.Accepted > 10_000, "the fuzz must reach the forwarding path");
        var updates = sink.Updates;
        Assert.Equal(updates.Count, core.Stats.Forwarded);
        Assert.NotEmpty(updates);
        foreach (var u in updates)
        {
            Assert.True(allowed.TryGetValue((u.Type, u.Number), out int max), $"unmapped element {u}");
            Assert.InRange(u.Value, 0, max);
        }
        // Change-only holds for any input.
        var last = new Dictionary<(char, int), int>();
        long resyncsSeen = core.Stats.Resyncs;
        int repeats = 0;
        foreach (var u in updates)
        {
            if (last.TryGetValue((u.Type, u.Number), out int v) && v == u.Value) repeats++;
            last[(u.Type, u.Number)] = u.Value;
        }
        // Only a resync may repeat a value, once per element.
        Assert.True(repeats <= resyncsSeen * allowed.Count, $"{repeats} repeated values with {resyncsSeen} resyncs");
    }

    [Fact]
    public void Stats_count_every_datagram_once()
    {
        var rig = new Rig();
        rig.Send(new CabinetState().FullState(1000));                       // 5 accepted
        rig.Send(Wire.Delta(1001, Wire.Co(3, true)));                        // accepted
        rig.Send(Wire.Delta(1002));                                          // accepted, empty
        rig.Send(Wire.Delta(900, Wire.Co(3, false)));                        // stale
        rig.Send(new byte[3]);                                               // rejected
        rig.Send(Wire.Delta(1003, Wire.Mag(1, 1f), Wire.Imp(3, 1f)));        // accepted, nothing mapped
        var s = rig.Core.Stats;
        Assert.Equal(8, s.Accepted);
        Assert.Equal(1, s.Stale);
        Assert.Equal(1, s.Rejected);
        Assert.Equal(9, s.Forwarded);
        Assert.Equal(1, s.Resyncs);
        Assert.Equal(9, rig.Take().Count);
    }
}
