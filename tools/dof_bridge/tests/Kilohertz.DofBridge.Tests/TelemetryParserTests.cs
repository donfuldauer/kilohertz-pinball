using System.Buffers.Binary;
using Xunit;

namespace Kilohertz.DofBridge.Tests;

public class TelemetryParserTests
{
    [Fact]
    public void ProtocolExampleParses()
    {
        byte[] d = Convert.FromHexString("504E544C010101008877665544332211" + "020001000000803F");
        Assert.Equal(ParseStatus.Ok, TelemetryParser.TryParse(d, out TelemetryHeader h));
        Assert.Equal(1, h.Version);
        Assert.True(h.IsFullState);
        Assert.Equal(1, h.EntryCount);
        Assert.Equal(0x1122334455667788UL, h.Tick);
        TelemetryEntry e = TelemetryParser.ReadEntry(d, 0);
        Assert.Equal(new TelemetryEntry(TelemetryKind.Lamp, 1, 1.0f), e);
    }

    [Fact]
    public void EmptyDatagramIsValid()
    {
        Assert.Equal(ParseStatus.Ok, TelemetryParser.TryParse(Dg.Make(7, false), out TelemetryHeader h));
        Assert.False(h.IsFullState);
        Assert.Equal(0, h.EntryCount);
    }

    [Fact]
    public void ReceiverChecks()
    {
        byte[] good = Dg.Make(5, false, Dg.Lamp(3, 0.5f), Dg.Coil(1, 1));
        Assert.Equal(ParseStatus.TooShort, TelemetryParser.TryParse(good.AsSpan(0, 15), out _));
        Assert.Equal(ParseStatus.TooShort, TelemetryParser.TryParse([], out _));

        byte[] magic = (byte[])good.Clone();
        magic[3] = (byte)'X';
        Assert.Equal(ParseStatus.BadMagic, TelemetryParser.TryParse(magic, out _));

        byte[] version = (byte[])good.Clone();
        version[4] = 2;
        Assert.Equal(ParseStatus.BadVersion, TelemetryParser.TryParse(version, out _));

        byte[] many = new byte[16 + (181 * 8)];
        good.AsSpan(0, 16).CopyTo(many);
        BinaryPrimitives.WriteUInt16LittleEndian(many.AsSpan(6), 181);
        Assert.Equal(ParseStatus.TooManyEntries, TelemetryParser.TryParse(many, out _));

        Assert.Equal(ParseStatus.LengthMismatch, TelemetryParser.TryParse(good.AsSpan(0, good.Length - 1), out _));
        byte[] longer = [.. good, 0];
        Assert.Equal(ParseStatus.LengthMismatch, TelemetryParser.TryParse(longer, out _));

        byte[] kind = (byte[])good.Clone();
        kind[16 + 8] = 6;
        Assert.Equal(ParseStatus.BadKind, TelemetryParser.TryParse(kind, out TelemetryHeader h));
        Assert.Equal(default, h);

        // Reserved byte and flag bits 1-7 are ignored by receivers.
        byte[] reserved = (byte[])good.Clone();
        reserved[17] = 0xAA;
        reserved[5] = 0xFE;
        Assert.Equal(ParseStatus.Ok, TelemetryParser.TryParse(reserved, out h));
        Assert.False(h.IsFullState);
    }

    [Fact]
    public void MaximumDatagramParses()
    {
        var entries = Enumerable.Range(0, 180).Select(i => Dg.Lamp((ushort)i, 0.25f)).ToArray();
        byte[] d = Dg.Make(1, true, entries);
        Assert.Equal(TelemetryParser.MaxDatagramLength, d.Length);
        Assert.Equal(ParseStatus.Ok, TelemetryParser.TryParse(d, out TelemetryHeader h));
        Assert.Equal(180, h.EntryCount);
        Assert.Equal(new TelemetryEntry(TelemetryKind.Lamp, 179, 0.25f), TelemetryParser.ReadEntry(d, 179));
    }

    /// <summary>Gate 5: random bytes never throw (seeded, bounded).</summary>
    [Fact]
    public void RandomBytesNeverThrow()
    {
        var rng = new Random(0x3535);
        var core = new BridgeCore(Fixtures.Map(), new NullDofSink());
        byte[] buf = new byte[1600];
        int ok = 0;
        for (int i = 0; i < 100_000; i++)
        {
            int len = rng.Next(0, buf.Length);
            rng.NextBytes(buf.AsSpan(0, len));
            if ((i & 1) == 0 && len >= 16)
            {
                // Make half of them plausible so the deeper checks run too.
                "PNTL"u8.CopyTo(buf);
                buf[4] = 1;
                int count = rng.Next(0, 200);
                BinaryPrimitives.WriteUInt16LittleEndian(buf.AsSpan(6), (ushort)count);
                if (rng.Next(2) == 0)
                {
                    len = Math.Min(buf.Length, 16 + (count * 8));
                    rng.NextBytes(buf.AsSpan(16, len - 16));
                    for (int k = 16; k < len; k += 8)
                    {
                        buf[k] %= 7;
                    }
                }
            }
            ParseStatus s = TelemetryParser.TryParse(buf.AsSpan(0, len), out TelemetryHeader h);
            if (s == ParseStatus.Ok)
            {
                ok++;
                for (int e = 0; e < h.EntryCount; e++)
                {
                    _ = TelemetryParser.ReadEntry(buf.AsSpan(0, len), e);
                }
            }
            core.Process(buf.AsSpan(0, len), 0);
        }
        Assert.True(ok > 500, $"only {ok} random datagrams were valid");
        Assert.Equal(100_000, core.Stats.Accepted + core.Stats.Rejected + core.Stats.Stale);
    }

    /// <summary>Gate 5: every single-byte mutation and truncation of real fixture datagrams is handled.</summary>
    [Fact]
    public void MutatedFixtureDatagramsNeverThrow()
    {
        var core = new BridgeCore(Fixtures.Map(), new NullDofSink());
        var rng = new Random(77);
        foreach (byte[] r in Fixtures.Records("faulty").Take(300))
        {
            byte[] m = (byte[])r.Clone();
            if (m.Length > 0)
            {
                m[rng.Next(m.Length)] ^= (byte)rng.Next(1, 256);
            }
            core.Process(m, 0);
            core.Process(r.AsSpan(0, rng.Next(r.Length + 1)), 0);
        }
        Assert.Equal(600, core.Stats.Accepted + core.Stats.Rejected + core.Stats.Stale);
    }
}
