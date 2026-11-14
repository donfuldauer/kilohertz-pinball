using Xunit;

namespace Kilohertz.DofBridge.Tests;

public class DofMapTests
{
    private const string Good = """
        { "format": "kilohertz-dof-map", "version": 1, "rom_name": "afm_113b", "table_name": "Attack",
          "extra": [1, 2],
          "entries": [
            { "source": "coil", "id": 3, "name": "left_flipper_coil", "type": "S", "number": 1 },
            { "source": "switch", "id": 255, "type": "W", "number": 65535, "future": true },
            { "source": "lamp", "id": 0, "name": "x", "type": "L", "number": 0 },
            { "source": "gi", "id": 31, "name": "gi", "type": "G", "number": 2 }
          ] }
        """;

    [Fact]
    public void ParsesGoodMap()
    {
        ResolvedDofMap m = ResolvedDofMap.Parse(Good);
        Assert.Equal("afm_113b", m.RomName);
        Assert.Equal("Attack", m.TableName);
        Assert.Equal(4, m.Entries.Count);
        Assert.Equal(new DofMapEntry(DofSourceKind.Coil, 3, "left_flipper_coil", 'S', 1), m.Entries[0]);
        Assert.Equal(new DofMapEntry(DofSourceKind.Switch, 255, "", 'W', 65535), m.Entries[1]);
        Assert.Equal(DofSourceKind.Gi, m.Entries[3].Source);
    }

    [Fact]
    public void MinimalMap()
    {
        ResolvedDofMap m = ResolvedDofMap.Parse("""{ "format": "kilohertz-dof-map", "version": 1, "rom_name": "r" }""");
        Assert.Empty(m.Entries);
        Assert.Equal("", m.TableName);
    }

    [Theory]
    [InlineData("not json")]
    [InlineData("[]")]
    [InlineData("""{ "version": 1, "rom_name": "r" }""")]
    [InlineData("""{ "format": "other", "version": 1, "rom_name": "r" }""")]
    [InlineData("""{ "format": "kilohertz-dof-map", "version": 2, "rom_name": "r" }""")]
    [InlineData("""{ "format": "kilohertz-dof-map", "version": "1", "rom_name": "r" }""")]
    [InlineData("""{ "format": "kilohertz-dof-map", "version": 1 }""")]
    [InlineData("""{ "format": "kilohertz-dof-map", "version": 1, "rom_name": 5 }""")]
    [InlineData("""{ "format": "kilohertz-dof-map", "version": 1, "rom_name": "r", "entries": {} }""")]
    [InlineData("""{ "format": "kilohertz-dof-map", "version": 1, "rom_name": "r", "entries": [1] }""")]
    [InlineData("""{ "format": "kilohertz-dof-map", "version": 1, "rom_name": "r", "entries": [ { "source": "magnet", "id": 1, "type": "S", "number": 1 } ] }""")]
    [InlineData("""{ "format": "kilohertz-dof-map", "version": 1, "rom_name": "r", "entries": [ { "source": "coil", "id": 256, "type": "S", "number": 1 } ] }""")]
    [InlineData("""{ "format": "kilohertz-dof-map", "version": 1, "rom_name": "r", "entries": [ { "source": "gi", "id": 32, "type": "G", "number": 1 } ] }""")]
    [InlineData("""{ "format": "kilohertz-dof-map", "version": 1, "rom_name": "r", "entries": [ { "source": "coil", "id": -1, "type": "S", "number": 1 } ] }""")]
    [InlineData("""{ "format": "kilohertz-dof-map", "version": 1, "rom_name": "r", "entries": [ { "source": "coil", "id": 1, "type": "X", "number": 1 } ] }""")]
    [InlineData("""{ "format": "kilohertz-dof-map", "version": 1, "rom_name": "r", "entries": [ { "source": "coil", "id": 1, "type": "SS", "number": 1 } ] }""")]
    [InlineData("""{ "format": "kilohertz-dof-map", "version": 1, "rom_name": "r", "entries": [ { "source": "coil", "id": 1, "type": "S", "number": 65536 } ] }""")]
    [InlineData("""{ "format": "kilohertz-dof-map", "version": 1, "rom_name": "r", "entries": [ { "source": "coil", "id": 1, "type": "S", "number": 1.5 } ] }""")]
    [InlineData("""{ "format": "kilohertz-dof-map", "version": 1, "rom_name": "r", "entries": [ { "source": "coil", "id": 1, "type": "S" } ] }""")]
    public void RejectsBadMaps(string json) => Assert.Throws<DofMapException>(() => ResolvedDofMap.Parse(json));

    [Fact]
    public void LoadMissingFileThrowsMapException()
    {
        var e = Assert.Throws<DofMapException>(() => ResolvedDofMap.Load(Path.Combine(TempDir.Create(), "nope.json")));
        Assert.NotNull(e.InnerException);
    }

    [Fact]
    public void LoadsFixtureWrittenByTheEngine()
    {
        ResolvedDofMap m = Fixtures.Map();
        Assert.Equal("dof_fixture_rom", m.RomName);
        Assert.Equal("DOF \"Fixture\" Table", m.TableName);
        Assert.Equal(12, m.Entries.Count);
        Assert.Contains(new DofMapEntry(DofSourceKind.Gi, 2, "gi_upper", 'G', 2), m.Entries);
        Assert.Contains(new DofMapEntry(DofSourceKind.Lamp, 60, "flasher_left", 'S', 60), m.Entries);
    }

    [Fact]
    public void ExceptionConstructors()
    {
        Assert.NotEmpty(new DofMapException().Message);
        Assert.NotEmpty(new BridgeUsageException().Message);
        Assert.NotNull(new BridgeUsageException("x", new InvalidOperationException()).InnerException);
    }
}
