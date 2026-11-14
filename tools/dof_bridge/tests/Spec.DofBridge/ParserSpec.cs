// Subphase 3.5 Independent Tester: the datagram parser against TELEMETRY_PROTOCOL.md sections 2 and 3.
using System.Buffers.Binary;
using Kilohertz.DofBridge;
using Xunit;

namespace Spec.DofBridge;

public class ParserSpec
{
    /// <summary>Reference check of protocol section 3 (independent of the bridge).</summary>
    internal static bool ReferenceAccepts(ReadOnlySpan<byte> d)
    {
        if (d.Length < 16) return false;
        if (d[0] != 0x50 || d[1] != 0x4E || d[2] != 0x54 || d[3] != 0x4C) return false;
        if (d[4] != 1) return false;
        int count = BinaryPrimitives.ReadUInt16LittleEndian(d[6..]);
        if (count > 180) return false;
        if (d.Length != 16 + 8 * count) return false;
        for (int i = 0; i < count; i++)
        {
            if (d[16 + 8 * i] > 5) return false;
        }
        return true;
    }

    [Fact]
    public void Constants_match_the_protocol()
    {
        Assert.Equal(16, TelemetryParser.HeaderLength);
        Assert.Equal(8, TelemetryParser.EntryLength);
        Assert.Equal(180, TelemetryParser.MaxEntries);
        Assert.Equal(0, (byte)TelemetryKind.Switch);
        Assert.Equal(1, (byte)TelemetryKind.Coil);
        Assert.Equal(2, (byte)TelemetryKind.Lamp);
        Assert.Equal(3, (byte)TelemetryKind.Gi);
        Assert.Equal(4, (byte)TelemetryKind.Magnet);
        Assert.Equal(5, (byte)TelemetryKind.HapticImpulse);
    }

    [Fact]
    public void Parses_the_example_of_the_protocol_document()
    {
        // TELEMETRY_PROTOCOL.md section 6: lamp 1 at full, tick 0x1122334455667788, full-state flag.
        byte[] d =
        {
            0x50, 0x4E, 0x54, 0x4C, 0x01, 0x01, 0x01, 0x00, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11,
            0x02, 0x00, 0x01, 0x00, 0x00, 0x00, 0x80, 0x3F,
        };
        Assert.Equal(ParseStatus.Ok, TelemetryParser.TryParse(d, out var h));
        Assert.Equal(1, h.Version);
        Assert.Equal(1, h.Flags);
        Assert.True(h.IsFullState);
        Assert.Equal(1, h.EntryCount);
        Assert.Equal(0x1122334455667788UL, h.Tick);
        Assert.Equal(new TelemetryEntry(TelemetryKind.Lamp, 1, 1.0f), TelemetryParser.ReadEntry(d, 0));
    }

    [Fact]
    public void Parses_every_kind_and_all_fields_little_endian()
    {
        var entries = new[]
        {
            new Wire.Entry(Wire.Switch, 255, 1f),
            new Wire.Entry(Wire.Coil, 0x0102, 0f),
            new Wire.Entry(Wire.Lamp, 7, 0.25f),
            new Wire.Entry(Wire.Gi, 31, 0.75f),
            new Wire.Entry(Wire.Magnet, 3, 0.5f),
            new Wire.Entry(Wire.Impulse, 0x2001, 0.125f),
        };
        var d = Wire.Datagram(0x0000_0001_0000_0002UL, false, entries);
        Assert.Equal(ParseStatus.Ok, TelemetryParser.TryParse(d, out var h));
        Assert.False(h.IsFullState);
        Assert.Equal(0, h.Flags);
        Assert.Equal(6, h.EntryCount);
        Assert.Equal(0x0000_0001_0000_0002UL, h.Tick);
        for (int i = 0; i < entries.Length; i++)
        {
            var e = TelemetryParser.ReadEntry(d, i);
            Assert.Equal((TelemetryKind)entries[i].Kind, e.Kind);
            Assert.Equal(entries[i].Id, e.Id);
            Assert.Equal(entries[i].Value, e.Value);
        }
    }

    [Fact]
    public void Accepts_the_boundaries()
    {
        // Header only.
        Assert.Equal(ParseStatus.Ok, TelemetryParser.TryParse(Wire.Datagram(0, false), out var h0));
        Assert.Equal(0, h0.EntryCount);
        // 180 entries = 1456 bytes.
        var max = Enumerable.Range(0, 180).Select(i => Wire.La(i, 1f)).ToArray();
        var d = Wire.Datagram(ulong.MaxValue, true, max);
        Assert.Equal(1456, d.Length);
        Assert.Equal(ParseStatus.Ok, TelemetryParser.TryParse(d, out var h));
        Assert.Equal(180, h.EntryCount);
        Assert.Equal(ulong.MaxValue, h.Tick);
        Assert.Equal(new TelemetryEntry(TelemetryKind.Lamp, 179, 1f), TelemetryParser.ReadEntry(d, 179));
    }

    [Fact]
    public void Reserved_entry_byte_is_ignored()
    {
        var d = Wire.Datagram(9, false, new Wire.Entry(Wire.Coil, 3, 1f, Reserved: 0xFF));
        Assert.Equal(ParseStatus.Ok, TelemetryParser.TryParse(d, out _));
        Assert.Equal(new TelemetryEntry(TelemetryKind.Coil, 3, 1f), TelemetryParser.ReadEntry(d, 0));
    }

    [Fact]
    public void Full_state_is_bit_0_of_flags_only()
    {
        // Section 3 lists every reason to reject; unknown flag bits are not one of them.
        var d = Wire.Raw(1, 0xFE, 1, 0, Array.Empty<Wire.Entry>());
        Assert.Equal(ParseStatus.Ok, TelemetryParser.TryParse(d, out var h));
        Assert.False(h.IsFullState);
        d = Wire.Raw(1, 0x81, 1, 0, Array.Empty<Wire.Entry>());
        Assert.Equal(ParseStatus.Ok, TelemetryParser.TryParse(d, out h));
        Assert.True(h.IsFullState);
    }

    [Theory]
    [InlineData(0)]
    [InlineData(1)]
    [InlineData(4)]
    [InlineData(15)]
    public void Rejects_short_datagrams(int length)
    {
        var d = Wire.Datagram(1, false).AsSpan(0, length).ToArray();
        Assert.Equal(ParseStatus.TooShort, TelemetryParser.TryParse(d, out _));
    }

    [Theory]
    [InlineData(0)]
    [InlineData(1)]
    [InlineData(2)]
    [InlineData(3)]
    public void Rejects_bad_magic(int corruptByte)
    {
        var d = Wire.Datagram(1, false, Wire.Co(3, true));
        d[corruptByte] ^= 0x20;
        Assert.Equal(ParseStatus.BadMagic, TelemetryParser.TryParse(d, out _));
    }

    [Theory]
    [InlineData(0)]
    [InlineData(2)]
    [InlineData(255)]
    public void Rejects_other_versions(byte version)
    {
        var d = Wire.Raw(1, 0, version, 1, new[] { Wire.Co(3, true) });
        Assert.Equal(ParseStatus.BadVersion, TelemetryParser.TryParse(d, out _));
    }

    [Theory]
    [InlineData(181)]
    [InlineData(182)]
    [InlineData(4096)]
    public void Rejects_more_than_180_entries(int count)
    {
        // Length is consistent with the count, so only the count is wrong.
        var entries = Enumerable.Range(0, count).Select(i => Wire.La(i % 256, 0f)).ToArray();
        var d = Wire.Raw(1, 0, 1, (ushort)count, entries);
        Assert.Equal(ParseStatus.TooManyEntries, TelemetryParser.TryParse(d, out _));
    }

    [Theory]
    [InlineData(2, 1)]   // declares more than it carries
    [InlineData(1, 2)]   // carries more than it declares
    [InlineData(0, 1)]
    [InlineData(180, 179)]
    public void Rejects_length_mismatch(int declared, int carried)
    {
        var entries = Enumerable.Range(0, carried).Select(i => Wire.La(i, 0f)).ToArray();
        var d = Wire.Raw(1, 0, 1, (ushort)declared, entries);
        Assert.Equal(ParseStatus.LengthMismatch, TelemetryParser.TryParse(d, out _));
    }

    [Theory]
    [InlineData(1)]
    [InlineData(7)]
    [InlineData(-1)]
    public void Rejects_truncated_or_padded_datagrams(int delta)
    {
        var d = Wire.Datagram(1, false, Wire.Co(3, true), Wire.Co(4, true));
        var changed = new byte[d.Length + delta];
        d.AsSpan(0, Math.Min(d.Length, changed.Length)).CopyTo(changed);
        Assert.Equal(ParseStatus.LengthMismatch, TelemetryParser.TryParse(changed, out _));
    }

    [Theory]
    [InlineData(6, 0)]
    [InlineData(6, 2)]
    [InlineData(255, 1)]
    [InlineData(128, 2)]
    public void Rejects_a_kind_above_5_in_any_entry(byte kind, int position)
    {
        var entries = new[] { Wire.Co(1, true), Wire.Co(2, true), Wire.Co(3, true) };
        entries[position] = new Wire.Entry(kind, 1, 1f);
        var d = Wire.Datagram(1, false, entries);
        Assert.Equal(ParseStatus.BadKind, TelemetryParser.TryParse(d, out _));
    }

    /// <summary>Gate 5: a bounded, seeded random-bytes run never throws and agrees with the protocol's accept rule.</summary>
    [Fact]
    public void Fuzz_random_bytes_never_throw()
    {
        var rng = new Random(0x3_5_2026);
        var buffer = new byte[1600];
        for (int i = 0; i < 300_000; i++)
        {
            int len = rng.Next(0, 4) switch
            {
                0 => rng.Next(0, 40),
                1 => 16 + 8 * rng.Next(0, 190),
                _ => rng.Next(0, buffer.Length + 1),
            };
            var d = buffer.AsSpan(0, len);
            rng.NextBytes(d);
            // Half of the runs get a plausible header so the deeper checks are reached.
            if (len >= 16 && (i & 1) == 0)
            {
                d[0] = 0x50; d[1] = 0x4E; d[2] = 0x54; d[3] = 0x4C;
                if (rng.Next(8) != 0) d[4] = 1;
                if (rng.Next(4) != 0) BinaryPrimitives.WriteUInt16LittleEndian(d[6..], (ushort)((len - 16) / 8));
                if (rng.Next(2) == 0)
                {
                    for (int e = 16; e < len; e += 8) d[e] = (byte)rng.Next(0, rng.Next(20) == 0 ? 8 : 6);
                }
            }
            CheckOne(d.ToArray(), i);
        }
    }

    /// <summary>Gate 5, structured: every single-byte corruption and every truncation of valid datagrams.</summary>
    [Fact]
    public void Fuzz_mutated_valid_datagrams_never_throw()
    {
        var rng = new Random(350);
        var state = new CabinetState();
        for (int i = 0; i < state.Lamps.Length; i++) state.Lamps[i] = (float)rng.NextDouble();
        var seeds = new List<byte[]>(state.FullState(1234));
        seeds.Add(Wire.Delta(77, Wire.Co(3, true), Wire.La(5, 0.5f), Wire.Imp(0x1001, 1f)));
        seeds.Add(Wire.Delta(78));
        int n = 0;
        foreach (var seed in seeds)
        {
            for (int cut = 0; cut <= seed.Length; cut += seed.Length > 200 ? 37 : 1)
            {
                CheckOne(seed.AsSpan(0, cut).ToArray(), n++);
            }
            for (int k = 0; k < 20_000; k++)
            {
                var m = (byte[])seed.Clone();
                int flips = 1 + rng.Next(3);
                for (int f = 0; f < flips; f++)
                {
                    // Bias towards the header and the kind bytes.
                    int at = rng.Next(3) == 0 ? rng.Next(Math.Min(16, m.Length)) : rng.Next(m.Length);
                    m[at] = (byte)rng.Next(256);
                }
                CheckOne(m, n++);
            }
        }
    }

    private static void CheckOne(byte[] d, int iteration)
    {
        ParseStatus status;
        TelemetryHeader header;
        try
        {
            status = TelemetryParser.TryParse(d, out header);
        }
        catch (Exception ex)
        {
            Assert.Fail($"TryParse threw {ex.GetType().Name} on iteration {iteration}: {Convert.ToHexString(d.AsSpan(0, Math.Min(d.Length, 48)))} (len {d.Length})");
            return;
        }
        bool want = ReferenceAccepts(d);
        if ((status == ParseStatus.Ok) != want)
        {
            Assert.Fail($"TryParse returned {status}, protocol says accept={want}, iteration {iteration}: {Convert.ToHexString(d.AsSpan(0, Math.Min(d.Length, 48)))} (len {d.Length})");
        }
        if (status != ParseStatus.Ok) return;
        Assert.Equal((d.Length - 16) / 8, header.EntryCount);
        Assert.Equal(BinaryPrimitives.ReadUInt64LittleEndian(d.AsSpan(8)), header.Tick);
        for (int i = 0; i < header.EntryCount; i++)
        {
            var e = TelemetryParser.ReadEntry(d, i);
            Assert.Equal(d[16 + 8 * i], (byte)e.Kind);
            Assert.Equal(BinaryPrimitives.ReadUInt16LittleEndian(d.AsSpan(18 + 8 * i)), e.Id);
            Assert.Equal(BitConverter.ToInt32(d, 20 + 8 * i), BitConverter.SingleToInt32Bits(e.Value));
        }
    }
}
