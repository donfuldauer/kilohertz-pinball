using System.Buffers.Binary;

namespace Kilohertz.DofBridge;

/// <summary>Telemetry entry kinds (docs/TELEMETRY_PROTOCOL.md §2.3).</summary>
public enum TelemetryKind : byte
{
    /// <summary>Switch state, 0 open / 1 closed.</summary>
    Switch = 0,
    /// <summary>Coil state, 0 off / 1 energised.</summary>
    Coil = 1,
    /// <summary>Lamp intensity 0..1.</summary>
    Lamp = 2,
    /// <summary>GI string intensity 0..1.</summary>
    Gi = 3,
    /// <summary>Magnet power 0..1 (not forwarded to DOF).</summary>
    Magnet = 4,
    /// <summary>Haptic impulse event (not forwarded to DOF).</summary>
    HapticImpulse = 5,
}

/// <summary>A parsed 16-byte datagram header.</summary>
/// <param name="Version">Protocol version (always 1 when parsed).</param>
/// <param name="Flags">Flags byte; bit 0 = full state.</param>
/// <param name="EntryCount">Number of 8-byte entries that follow.</param>
/// <param name="Tick">The sim tick the entries describe.</param>
public readonly record struct TelemetryHeader(byte Version, byte Flags, ushort EntryCount, ulong Tick)
{
    /// <summary>Whether flags bit 0 (full state) is set.</summary>
    public bool IsFullState => (Flags & TelemetryParser.FlagFullState) != 0;
}

/// <summary>One decoded 8-byte entry.</summary>
/// <param name="Kind">Entry kind.</param>
/// <param name="Id">Element id.</param>
/// <param name="Value">Element value.</param>
public readonly record struct TelemetryEntry(TelemetryKind Kind, ushort Id, float Value);

/// <summary>Outcome of <see cref="TelemetryParser.TryParse"/>.</summary>
public enum ParseStatus
{
    /// <summary>The datagram is valid.</summary>
    Ok,
    /// <summary>Shorter than the 16-byte header.</summary>
    TooShort,
    /// <summary>Magic is not "PNTL".</summary>
    BadMagic,
    /// <summary>Version is not 1.</summary>
    BadVersion,
    /// <summary>Entry count above 180.</summary>
    TooManyEntries,
    /// <summary>Length differs from 16 + 8 × entry count.</summary>
    LengthMismatch,
    /// <summary>An entry has a kind above 5.</summary>
    BadKind,
}

/// <summary>
/// Allocation-free parser for UDP telemetry v1 datagrams. Implements every receiver check of
/// docs/TELEMETRY_PROTOCOL.md §3 and never throws, whatever the input bytes.
/// </summary>
public static class TelemetryParser
{
    /// <summary>Header length in bytes.</summary>
    public const int HeaderLength = 16;
    /// <summary>Entry length in bytes.</summary>
    public const int EntryLength = 8;
    /// <summary>Maximum entries per datagram.</summary>
    public const int MaxEntries = 180;
    /// <summary>Protocol version understood by this parser.</summary>
    public const byte Version = 1;
    /// <summary>Flags bit 0: full state.</summary>
    public const byte FlagFullState = 1;
    /// <summary>Largest valid datagram, bytes.</summary>
    public const int MaxDatagramLength = HeaderLength + (MaxEntries * EntryLength);

    /// <summary>Validates <paramref name="datagram"/> and reads its header.</summary>
    /// <param name="datagram">Raw datagram bytes.</param>
    /// <param name="header">The header when the result is <see cref="ParseStatus.Ok"/>; default otherwise.</param>
    /// <returns>The first failed check, or <see cref="ParseStatus.Ok"/>.</returns>
    public static ParseStatus TryParse(ReadOnlySpan<byte> datagram, out TelemetryHeader header)
    {
        header = default;
        if (datagram.Length < HeaderLength)
        {
            return ParseStatus.TooShort;
        }
        if (datagram[0] != (byte)'P' || datagram[1] != (byte)'N' || datagram[2] != (byte)'T' || datagram[3] != (byte)'L')
        {
            return ParseStatus.BadMagic;
        }
        if (datagram[4] != Version)
        {
            return ParseStatus.BadVersion;
        }
        ushort count = BinaryPrimitives.ReadUInt16LittleEndian(datagram.Slice(6, 2));
        if (count > MaxEntries)
        {
            return ParseStatus.TooManyEntries;
        }
        if (datagram.Length != HeaderLength + (count * EntryLength))
        {
            return ParseStatus.LengthMismatch;
        }
        for (int i = 0; i < count; i++)
        {
            if (datagram[HeaderLength + (i * EntryLength)] > (byte)TelemetryKind.HapticImpulse)
            {
                return ParseStatus.BadKind;
            }
        }
        header = new TelemetryHeader(
            datagram[4],
            datagram[5],
            count,
            BinaryPrimitives.ReadUInt64LittleEndian(datagram.Slice(8, 8)));
        return ParseStatus.Ok;
    }

    /// <summary>Reads entry <paramref name="index"/> of a datagram that parsed <see cref="ParseStatus.Ok"/>.</summary>
    /// <param name="datagram">The datagram bytes.</param>
    /// <param name="index">Entry index, below the header's entry count.</param>
    /// <returns>The decoded entry.</returns>
    public static TelemetryEntry ReadEntry(ReadOnlySpan<byte> datagram, int index)
    {
        ReadOnlySpan<byte> e = datagram.Slice(HeaderLength + (index * EntryLength), EntryLength);
        return new TelemetryEntry(
            (TelemetryKind)e[0],
            BinaryPrimitives.ReadUInt16LittleEndian(e.Slice(2, 2)),
            BinaryPrimitives.ReadSingleLittleEndian(e.Slice(4, 4)));
    }
}
