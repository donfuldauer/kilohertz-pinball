namespace Kilohertz.DofBridge;

/// <summary>
/// Finds the user's DirectOutput.dll. Search order: the explicit path (when given, nothing else is
/// tried), the <c>KHZ_DOF_PATH</c> environment variable, then <see cref="DefaultSearchDirectories"/>.
/// A directory is probed for an architecture subfolder (<c>x64</c> or <c>x86</c>, matching the
/// process) before the directory itself. Unverified on hardware (D33).
/// </summary>
public static class DofLocator
{
    /// <summary>The DOF assembly file name.</summary>
    public const string DllName = "DirectOutput.dll";

    /// <summary>Environment variable naming a DOF directory or DLL.</summary>
    public const string EnvironmentVariable = "KHZ_DOF_PATH";

    /// <summary>Finds DirectOutput.dll with the process environment and default directories.</summary>
    /// <param name="explicitPath">A directory or DLL path from <c>--dof-path</c>, or null.</param>
    /// <returns>The DLL path, or null when not found.</returns>
    public static string? FindDirectOutputDll(string? explicitPath) =>
        FindDirectOutputDll(explicitPath, Environment.GetEnvironmentVariable, DefaultSearchDirectories());

    /// <summary>Finds DirectOutput.dll with an injected environment and search list.</summary>
    /// <param name="explicitPath">A directory or DLL path, or null.</param>
    /// <param name="environment">Environment lookup.</param>
    /// <param name="searchDirectories">Directories tried last, in order.</param>
    /// <returns>The DLL path, or null when not found.</returns>
    public static string? FindDirectOutputDll(string? explicitPath, Func<string, string?> environment, IEnumerable<string> searchDirectories)
    {
        ArgumentNullException.ThrowIfNull(environment);
        ArgumentNullException.ThrowIfNull(searchDirectories);
        if (!string.IsNullOrEmpty(explicitPath))
        {
            return Probe(explicitPath);
        }
        string? env = environment(EnvironmentVariable);
        if (!string.IsNullOrEmpty(env) && Probe(env) is string fromEnv)
        {
            return fromEnv;
        }
        foreach (string dir in searchDirectories)
        {
            if (Probe(dir) is string found)
            {
                return found;
            }
        }
        return null;
    }

    /// <summary>
    /// Install locations tried last: registry values (Windows; best effort) and
    /// <c>C:\DirectOutput</c>, <c>%ProgramFiles%\DirectOutput</c>, <c>%ProgramFiles(x86)%\DirectOutput</c>.
    /// </summary>
    /// <returns>Directories in search order (empty off Windows).</returns>
    public static IReadOnlyList<string> DefaultSearchDirectories()
    {
        var dirs = new List<string>();
        if (!OperatingSystem.IsWindows())
        {
            return dirs;
        }
        foreach (string key in new[] { @"SOFTWARE\DirectOutput", @"SOFTWARE\WOW6432Node\DirectOutput" })
        {
            try
            {
                using Microsoft.Win32.RegistryKey? k = Microsoft.Win32.Registry.LocalMachine.OpenSubKey(key);
                if (k?.GetValue("InstallPath") is string p && p.Length > 0)
                {
                    dirs.Add(p);
                }
            }
            catch (Exception e) when (e is System.Security.SecurityException or UnauthorizedAccessException or IOException)
            {
                // Unreadable key: skip.
            }
        }
        dirs.Add(@"C:\DirectOutput");
        foreach (Environment.SpecialFolder f in new[] { Environment.SpecialFolder.ProgramFiles, Environment.SpecialFolder.ProgramFilesX86 })
        {
            string pf = Environment.GetFolderPath(f);
            if (pf.Length > 0)
            {
                dirs.Add(Path.Combine(pf, "DirectOutput"));
            }
        }
        return dirs;
    }

    private static string? Probe(string path)
    {
        if (File.Exists(path))
        {
            return path;
        }
        if (!Directory.Exists(path))
        {
            return null;
        }
        string arch = Environment.Is64BitProcess ? "x64" : "x86";
        foreach (string candidate in new[] { Path.Combine(path, arch, DllName), Path.Combine(path, DllName) })
        {
            if (File.Exists(candidate))
            {
                return candidate;
            }
        }
        return null;
    }
}
