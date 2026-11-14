// Subphase 3.5 Independent Tester: the resolved-mapping file (published format "kilohertz-dof-map" v1).
using Kilohertz.DofBridge;
using Xunit;

namespace Spec.DofBridge;

public class MapSpec
{
    private const string Sample = """
        { "format": "kilohertz-dof-map", "version": 1, "rom_name": "apexcircuit", "table_name": "Apex Circuit",
          "entries": [ { "source": "coil", "id": 3, "name": "left_flipper_coil", "type": "S", "number": 1 } ] }
        """;

    private static string With(string entry) =>
        "{ \"format\": \"kilohertz-dof-map\", \"version\": 1, \"rom_name\": \"r\", \"table_name\": \"t\", \"entries\": [ " + entry + " ] }";

    [Fact]
    public void Parses_the_documented_example()
    {
        var map = ResolvedDofMap.Parse(Sample);
        Assert.Equal("apexcircuit", map.RomName);
        Assert.Equal("Apex Circuit", map.TableName);
        Assert.Equal(new[] { new DofMapEntry(DofSourceKind.Coil, 3, "left_flipper_coil", 'S', 1) }, map.Entries);
    }

    [Fact]
    public void Parses_every_source_and_type_and_keeps_order_and_duplicates()
    {
        var b = new MapBuilder { Rom = "rom2", Table = "Table Two" }
            .Add("switch", 0, 'W', 0, "a")
            .Add("coil", 255, 'S', 65535, "b")
            .Add("lamp", 255, 'L', 1, "c")
            .Add("gi", 31, 'G', 2, "d")
            .Add("switch", 255, 'E', 3, "e")
            .Add("gi", 0, 'L', 4, "f")
            .Add("coil", 255, 'S', 48, "b");
        var map = b.Build();
        Assert.Equal("rom2", map.RomName);
        Assert.Equal("Table Two", map.TableName);
        Assert.Equal(new[]
        {
            new DofMapEntry(DofSourceKind.Switch, 0, "a", 'W', 0),
            new DofMapEntry(DofSourceKind.Coil, 255, "b", 'S', 65535),
            new DofMapEntry(DofSourceKind.Lamp, 255, "c", 'L', 1),
            new DofMapEntry(DofSourceKind.Gi, 31, "d", 'G', 2),
            new DofMapEntry(DofSourceKind.Switch, 255, "e", 'E', 3),
            new DofMapEntry(DofSourceKind.Gi, 0, "f", 'L', 4),
            new DofMapEntry(DofSourceKind.Coil, 255, "b", 'S', 48),
        }, map.Entries);
    }

    [Fact]
    public void Empty_entries_and_unknown_properties_are_fine()
    {
        var map = ResolvedDofMap.Parse(
            "{ \"format\": \"kilohertz-dof-map\", \"version\": 1, \"future\": [1, {\"x\": null}], \"rom_name\": \"r\", \"table_name\": \"t\", \"entries\": [] }");
        Assert.Empty(map.Entries);
        map = ResolvedDofMap.Parse(With("{ \"source\": \"lamp\", \"comment\": \"hi\", \"id\": 9, \"name\": \"n\", \"type\": \"L\", \"number\": 4, \"extra\": {} }"));
        Assert.Equal(new[] { new DofMapEntry(DofSourceKind.Lamp, 9, "n", 'L', 4) }, map.Entries);
    }

    [Theory]
    [InlineData("")]
    [InlineData("   ")]
    [InlineData("not json")]
    [InlineData("[]")]
    [InlineData("null")]
    [InlineData("42")]
    [InlineData("{")]
    [InlineData("{}")]
    [InlineData("{ \"format\": \"other-map\", \"version\": 1, \"rom_name\": \"r\", \"table_name\": \"t\", \"entries\": [] }")]
    [InlineData("{ \"format\": \"kilohertz-dof-map\", \"version\": 2, \"rom_name\": \"r\", \"table_name\": \"t\", \"entries\": [] }")]
    [InlineData("{ \"format\": \"kilohertz-dof-map\", \"version\": 0, \"rom_name\": \"r\", \"table_name\": \"t\", \"entries\": [] }")]
    [InlineData("{ \"format\": \"kilohertz-dof-map\", \"version\": \"1\", \"rom_name\": \"r\", \"table_name\": \"t\", \"entries\": [] }")]
    [InlineData("{ \"version\": 1, \"rom_name\": \"r\", \"table_name\": \"t\", \"entries\": [] }")]
    [InlineData("{ \"format\": \"kilohertz-dof-map\", \"rom_name\": \"r\", \"table_name\": \"t\", \"entries\": [] }")]
    [InlineData("{ \"format\": \"kilohertz-dof-map\", \"version\": 1, \"rom_name\": \"r\", \"table_name\": \"t\", \"entries\": {} }")]
    [InlineData("{ \"format\": \"kilohertz-dof-map\", \"version\": 1, \"rom_name\": \"r\", \"table_name\": \"t\", \"entries\": [ 7 ] }")]
    public void Rejects_documents_that_are_not_a_v1_map(string json)
    {
        Assert.Throws<DofMapException>(() => ResolvedDofMap.Parse(json));
    }

    [Theory]
    // unknown source / type
    [InlineData("{ \"source\": \"magnet\", \"id\": 1, \"name\": \"n\", \"type\": \"S\", \"number\": 1 }")]
    [InlineData("{ \"source\": \"\", \"id\": 1, \"name\": \"n\", \"type\": \"S\", \"number\": 1 }")]
    [InlineData("{ \"source\": 1, \"id\": 1, \"name\": \"n\", \"type\": \"S\", \"number\": 1 }")]
    [InlineData("{ \"id\": 1, \"name\": \"n\", \"type\": \"S\", \"number\": 1 }")]
    [InlineData("{ \"source\": \"coil\", \"id\": 1, \"name\": \"n\", \"type\": \"X\", \"number\": 1 }")]
    [InlineData("{ \"source\": \"coil\", \"id\": 1, \"name\": \"n\", \"type\": \"SS\", \"number\": 1 }")]
    [InlineData("{ \"source\": \"coil\", \"id\": 1, \"name\": \"n\", \"type\": \"\", \"number\": 1 }")]
    [InlineData("{ \"source\": \"coil\", \"id\": 1, \"name\": \"n\", \"number\": 1 }")]
    // id out of the telemetry range
    [InlineData("{ \"source\": \"coil\", \"id\": 256, \"name\": \"n\", \"type\": \"S\", \"number\": 1 }")]
    [InlineData("{ \"source\": \"switch\", \"id\": 256, \"name\": \"n\", \"type\": \"W\", \"number\": 1 }")]
    [InlineData("{ \"source\": \"lamp\", \"id\": 256, \"name\": \"n\", \"type\": \"L\", \"number\": 1 }")]
    [InlineData("{ \"source\": \"gi\", \"id\": 32, \"name\": \"n\", \"type\": \"G\", \"number\": 1 }")]
    [InlineData("{ \"source\": \"gi\", \"id\": 255, \"name\": \"n\", \"type\": \"G\", \"number\": 1 }")]
    [InlineData("{ \"source\": \"coil\", \"id\": -1, \"name\": \"n\", \"type\": \"S\", \"number\": 1 }")]
    [InlineData("{ \"source\": \"coil\", \"id\": 65539, \"name\": \"n\", \"type\": \"S\", \"number\": 1 }")]
    [InlineData("{ \"source\": \"coil\", \"id\": 4294967299, \"name\": \"n\", \"type\": \"S\", \"number\": 1 }")]
    [InlineData("{ \"source\": \"coil\", \"id\": 1.5, \"name\": \"n\", \"type\": \"S\", \"number\": 1 }")]
    [InlineData("{ \"source\": \"coil\", \"id\": \"3\", \"name\": \"n\", \"type\": \"S\", \"number\": 1 }")]
    [InlineData("{ \"source\": \"coil\", \"name\": \"n\", \"type\": \"S\", \"number\": 1 }")]
    // number out of 0..=65535
    [InlineData("{ \"source\": \"coil\", \"id\": 1, \"name\": \"n\", \"type\": \"S\", \"number\": -1 }")]
    [InlineData("{ \"source\": \"coil\", \"id\": 1, \"name\": \"n\", \"type\": \"S\", \"number\": 65536 }")]
    [InlineData("{ \"source\": \"coil\", \"id\": 1, \"name\": \"n\", \"type\": \"S\", \"number\": 4294967297 }")]
    [InlineData("{ \"source\": \"coil\", \"id\": 1, \"name\": \"n\", \"type\": \"S\", \"number\": 0.5 }")]
    [InlineData("{ \"source\": \"coil\", \"id\": 1, \"name\": \"n\", \"type\": \"S\" }")]
    public void Rejects_bad_entries(string entry)
    {
        Assert.Throws<DofMapException>(() => ResolvedDofMap.Parse(With(entry)));
        // A bad entry is an error wherever it stands.
        var good = "{ \"source\": \"coil\", \"id\": 1, \"name\": \"n\", \"type\": \"S\", \"number\": 1 }";
        Assert.Throws<DofMapException>(() => ResolvedDofMap.Parse(With(good + ", " + entry)));
    }

    [Fact]
    public void Load_reads_a_utf8_file_and_reports_a_missing_one()
    {
        var dir = Directory.CreateTempSubdirectory("spec_dof_map_");
        try
        {
            var path = Path.Combine(dir.FullName, "map.json");
            var b = new MapBuilder { Rom = "romé", Table = "Tâble ☄" }.Add("gi", 4, 'G', 9, "général");
            File.WriteAllText(path, b.Json(), new System.Text.UTF8Encoding(false));
            var map = ResolvedDofMap.Load(path);
            Assert.Equal("romé", map.RomName);
            Assert.Equal("Tâble ☄", map.TableName);
            Assert.Equal(new[] { new DofMapEntry(DofSourceKind.Gi, 4, "général", 'G', 9) }, map.Entries);

            Assert.Throws<DofMapException>(() => ResolvedDofMap.Load(Path.Combine(dir.FullName, "missing.json")));
            File.WriteAllBytes(path, new byte[] { 0xFF, 0xFE, 0x00, 0x7B });
            Assert.Throws<DofMapException>(() => ResolvedDofMap.Load(path));
        }
        finally
        {
            dir.Delete(true);
        }
    }

    /// <summary>The file the engine writes (`pin_app::dof::ResolvedDofMap::to_json`, checked in by the Rust spec test).</summary>
    [Fact]
    public void Loads_the_file_the_engine_writes()
    {
        var map = ResolvedDofMap.Load(Path.Combine(Fixtures.Dir, "engine_map.json"));
        Assert.Equal("apexcircuit", map.RomName);
        Assert.Equal("Apex Circuit", map.TableName);
        Assert.Equal(new[]
        {
            new DofMapEntry(DofSourceKind.Coil, 1, "left_flipper_coil", 'S', 1),
            new DofMapEntry(DofSourceKind.Switch, 24, "right_orbit_opto", 'W', 24),
            new DofMapEntry(DofSourceKind.Lamp, 40, "overdrive_ready", 'L', 140),
            new DofMapEntry(DofSourceKind.Gi, 7, "gi_upper", 'G', 2),
            new DofMapEntry(DofSourceKind.Coil, 255, "knocker_coil", 'E', 65535),
            new DofMapEntry(DofSourceKind.Coil, 1, "left_flipper_coil", 'S', 48),
            new DofMapEntry(DofSourceKind.Gi, 31, "gi_edge", 'L', 9),
        }, map.Entries);
    }

    /// <summary>Gate 5 (parsers): the map parser fails only with DofMapException on arbitrary input.</summary>
    [Fact]
    public void Fuzz_map_parser_only_throws_DofMapException()
    {
        var rng = new Random(909);
        string seed = MapBuilder.Standard().Json();
        string alphabet = "{}[]:,\"\\ \n-0123456789.eE+truefalsnl" + "sourceidnametypenumberformatversionentriesSWLGEcoilswitchlampgié☄\0";
        for (int i = 0; i < 60_000; i++)
        {
            string input;
            switch (rng.Next(3))
            {
                case 0:
                {
                    var chars = seed.ToCharArray();
                    int edits = 1 + rng.Next(4);
                    for (int e = 0; e < edits; e++) chars[rng.Next(chars.Length)] = alphabet[rng.Next(alphabet.Length)];
                    input = new string(chars);
                    break;
                }
                case 1:
                    input = seed[..rng.Next(seed.Length + 1)];
                    break;
                default:
                {
                    var chars = new char[rng.Next(0, 200)];
                    for (int c = 0; c < chars.Length; c++) chars[c] = rng.Next(5) == 0 ? (char)rng.Next(0, 0x3000) : alphabet[rng.Next(alphabet.Length)];
                    input = new string(chars);
                    break;
                }
            }
            try
            {
                var map = ResolvedDofMap.Parse(input);
                // Whatever is accepted obeys the documented ranges.
                Assert.NotNull(map.Entries);
                foreach (var e in map.Entries)
                {
                    Assert.InRange(e.Id, 0, e.Source == DofSourceKind.Gi ? 31 : 255);
                    Assert.InRange(e.Number, 0, 65535);
                    Assert.Contains(e.Type, "SWLGE");
                }
            }
            catch (DofMapException)
            {
            }
            catch (Exception ex) when (ex is not Xunit.Sdk.XunitException)
            {
                Assert.Fail($"Parse threw {ex.GetType().Name} on iteration {i}: {input}");
            }
        }
    }

    [Fact]
    public void Deeply_nested_input_is_an_error_not_a_crash()
    {
        Assert.Throws<DofMapException>(() => ResolvedDofMap.Parse(new string('[', 100_000)));
        Assert.Throws<DofMapException>(() => ResolvedDofMap.Parse(
            "{ \"format\": \"kilohertz-dof-map\", \"version\": 1, \"rom_name\": \"r\", \"table_name\": \"t\", \"entries\": " + new string('[', 5_000) + new string(']', 5_000) + " }"));
    }
}
