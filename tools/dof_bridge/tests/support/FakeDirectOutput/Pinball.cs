namespace DirectOutput;

/// <summary>Stand-in for DirectOutput.Pinball: records every call in <see cref="Log"/>.</summary>
public class Pinball
{
    /// <summary>Calls made on any instance, in order.</summary>
    public static List<string> Log { get; } = [];

    /// <summary>Same parameter names and order as DOF.</summary>
    public void Setup(string GlobalConfigFilename = "", string TableFilename = "", string RomName = "")
    {
        lock (Log)
        {
            Log.Add($"Setup({GlobalConfigFilename}|{TableFilename}|{RomName})");
        }
    }

    /// <summary>Init.</summary>
    public void Init()
    {
        lock (Log)
        {
            Log.Add("Init");
        }
    }

    /// <summary>Table-element update.</summary>
    public void ReceiveData(char TableElementTypeChar, int Number, int Value)
    {
        lock (Log)
        {
            Log.Add($"{TableElementTypeChar} {Number} {Value}");
        }
    }

    /// <summary>Finish.</summary>
    public void Finish()
    {
        lock (Log)
        {
            Log.Add("Finish");
        }
    }
}
