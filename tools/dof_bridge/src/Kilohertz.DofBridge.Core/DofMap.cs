using System.Text.Json;

namespace Kilohertz.DofBridge;

/// <summary>Telemetry element kinds a DOF mapping can name (telemetry kinds 0–3).</summary>
public enum DofSourceKind
{
    /// <summary>Switch (telemetry kind 0).</summary>
    Switch = 0,
    /// <summary>Coil (telemetry kind 1).</summary>
    Coil = 1,
    /// <summary>Lamp (telemetry kind 2).</summary>
    Lamp = 2,
    /// <summary>GI string (telemetry kind 3).</summary>
    Gi = 3,
}

/// <summary>One resolved mapping: telemetry element → DOF table element.</summary>
/// <param name="Source">Telemetry element kind.</param>
/// <param name="Id">Telemetry id (0..255; GI 0..31).</param>
/// <param name="Name">The table's element name (diagnostics only).</param>
/// <param name="Type">DOF table-element type char: S, W, L, G or E.</param>
/// <param name="Number">DOF table-element number (0..65535).</param>
public sealed record DofMapEntry(DofSourceKind Source, int Id, string Name, char Type, int Number);

/// <summary>A resolved-mapping file is malformed.</summary>
public sealed class DofMapException : Exception
{
    /// <summary>Creates the exception.</summary>
    /// <param name="message">What is wrong.</param>
    public DofMapException(string message) : base(message) { }

    /// <summary>Creates the exception with an inner cause.</summary>
    /// <param name="message">What is wrong.</param>
    /// <param name="inner">Cause.</param>
    public DofMapException(string message, Exception inner) : base(message, inner) { }

    /// <summary>Creates the exception.</summary>
    public DofMapException() : base("invalid DOF map") { }
}

/// <summary>
/// The resolved id → DOF element map the engine writes at table load
/// (format "kilohertz-dof-map", version 1; see DECISIONS.md "3.5 — DOF bridge public API").
/// </summary>
public sealed class ResolvedDofMap
{
    /// <summary>The <c>format</c> value every map file carries.</summary>
    public const string FormatName = "kilohertz-dof-map";
    /// <summary>The only supported <c>version</c>.</summary>
    public const int FormatVersion = 1;

    /// <summary>Creates a map.</summary>
    /// <param name="romName">ROM name DOF configs are keyed by.</param>
    /// <param name="tableName">Table display name.</param>
    /// <param name="entries">Mappings.</param>
    public ResolvedDofMap(string romName, string tableName, IReadOnlyList<DofMapEntry> entries)
    {
        RomName = romName;
        TableName = tableName;
        Entries = entries;
    }

    /// <summary>ROM name DOF configs are keyed by.</summary>
    public string RomName { get; }

    /// <summary>Table display name.</summary>
    public string TableName { get; }

    /// <summary>All mappings, in file order.</summary>
    public IReadOnlyList<DofMapEntry> Entries { get; }

    /// <summary>Reads and parses a map file.</summary>
    /// <param name="path">File path.</param>
    /// <returns>The map.</returns>
    /// <exception cref="DofMapException">Unreadable or invalid file.</exception>
    public static ResolvedDofMap Load(string path)
    {
        string text;
        try
        {
            text = File.ReadAllText(path);
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or ArgumentException or NotSupportedException)
        {
            throw new DofMapException($"cannot read DOF map '{path}': {e.Message}", e);
        }
        return Parse(text);
    }

    /// <summary>Parses map JSON.</summary>
    /// <param name="json">JSON text.</param>
    /// <returns>The map.</returns>
    /// <exception cref="DofMapException">Invalid JSON or content.</exception>
    public static ResolvedDofMap Parse(string json)
    {
        try
        {
            using JsonDocument doc = JsonDocument.Parse(json);
            JsonElement root = doc.RootElement;
            if (root.ValueKind != JsonValueKind.Object)
            {
                throw new DofMapException("DOF map: top level must be an object");
            }
            string format = RequiredString(root, "format");
            if (format != FormatName)
            {
                throw new DofMapException($"DOF map: format must be '{FormatName}', found '{format}'");
            }
            int version = RequiredInt(root, "version");
            if (version != FormatVersion)
            {
                throw new DofMapException($"DOF map: unsupported version {version}");
            }
            string rom = RequiredString(root, "rom_name");
            string table = OptionalString(root, "table_name") ?? string.Empty;
            var entries = new List<DofMapEntry>();
            if (root.TryGetProperty("entries", out JsonElement list))
            {
                if (list.ValueKind != JsonValueKind.Array)
                {
                    throw new DofMapException("DOF map: entries must be an array");
                }
                int i = 0;
                foreach (JsonElement e in list.EnumerateArray())
                {
                    entries.Add(ParseEntry(e, i));
                    i++;
                }
            }
            return new ResolvedDofMap(rom, table, entries);
        }
        catch (JsonException e)
        {
            throw new DofMapException($"DOF map: invalid JSON: {e.Message}", e);
        }
    }

    private static DofMapEntry ParseEntry(JsonElement e, int index)
    {
        if (e.ValueKind != JsonValueKind.Object)
        {
            throw new DofMapException($"DOF map: entries[{index}] must be an object");
        }
        string sourceText = RequiredString(e, "source");
        DofSourceKind source = sourceText switch
        {
            "switch" => DofSourceKind.Switch,
            "coil" => DofSourceKind.Coil,
            "lamp" => DofSourceKind.Lamp,
            "gi" => DofSourceKind.Gi,
            _ => throw new DofMapException($"DOF map: entries[{index}].source '{sourceText}' is not switch, coil, lamp or gi"),
        };
        int id = RequiredInt(e, "id");
        int maxId = source == DofSourceKind.Gi ? 31 : 255;
        if (id < 0 || id > maxId)
        {
            throw new DofMapException($"DOF map: entries[{index}].id {id} is outside 0..{maxId}");
        }
        string typeText = RequiredString(e, "type");
        if (typeText.Length != 1 || "SWLGE".IndexOf(typeText[0], StringComparison.Ordinal) < 0)
        {
            throw new DofMapException($"DOF map: entries[{index}].type '{typeText}' is not S, W, L, G or E");
        }
        int number = RequiredInt(e, "number");
        if (number < 0 || number > ushort.MaxValue)
        {
            throw new DofMapException($"DOF map: entries[{index}].number {number} is outside 0..65535");
        }
        string name = OptionalString(e, "name") ?? string.Empty;
        return new DofMapEntry(source, id, name, typeText[0], number);
    }

    private static string RequiredString(JsonElement o, string name)
    {
        return OptionalString(o, name) ?? throw new DofMapException($"DOF map: missing string '{name}'");
    }

    private static string? OptionalString(JsonElement o, string name)
    {
        if (!o.TryGetProperty(name, out JsonElement v))
        {
            return null;
        }
        if (v.ValueKind != JsonValueKind.String)
        {
            throw new DofMapException($"DOF map: '{name}' must be a string");
        }
        return v.GetString();
    }

    private static int RequiredInt(JsonElement o, string name)
    {
        if (!o.TryGetProperty(name, out JsonElement v) || v.ValueKind != JsonValueKind.Number || !v.TryGetInt32(out int n))
        {
            throw new DofMapException($"DOF map: missing or non-integer '{name}'");
        }
        return n;
    }
}
