using System.Runtime.Versioning;

namespace CodexMiMoLauncher;

internal static class Program
{
    [STAThread]
    [SupportedOSPlatform("windows")]
    private static void Main()
    {
        ApplicationConfiguration.Initialize();
        Application.Run(new LauncherForm());
    }
}
