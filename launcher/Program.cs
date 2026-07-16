using System.Runtime.Versioning;
using System.Security.Cryptography;
using System.Security.Principal;
using System.Text;

namespace CodexMiMoLauncher;

internal static class Program
{
    [STAThread]
    [SupportedOSPlatform("windows")]
    private static void Main()
    {
        var instanceScope = WindowsIdentity.GetCurrent().User?.Value ?? Environment.UserName;
        var scopeHash = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(instanceScope)));
        // Local keeps the objects in this interactive Windows session, while the SID-derived
        // name prevents one Windows user from signalling another user's launcher.
        var mutexName = $"Local\\CodexMiMoLauncher-{scopeHash}";
        var activationEventName = $"Local\\CodexMiMoLauncher-Activate-{scopeHash}";
        using var instanceMutex = new Mutex(initiallyOwned: true, mutexName, out var isFirstInstance);
        if (!isFirstInstance)
        {
            SignalExistingInstance(activationEventName);
            return;
        }

        try
        {
            using var activationEvent = new EventWaitHandle(false, EventResetMode.AutoReset, activationEventName);
            ApplicationConfiguration.Initialize();
            using var form = new LauncherForm();
            var activationRegistration = ThreadPool.RegisterWaitForSingleObject(
                activationEvent,
                static (state, timedOut) =>
                {
                    if (!timedOut && state is LauncherForm launcher)
                        launcher.ActivateFromAnotherInstance();
                },
                form,
                Timeout.Infinite,
                executeOnlyOnce: false);
            try
            {
                Application.Run(form);
            }
            finally
            {
                activationRegistration.Unregister(null);
            }
        }
        finally
        {
            instanceMutex.ReleaseMutex();
        }
    }

    private static void SignalExistingInstance(string activationEventName)
    {
        // A second process can observe the mutex just before the first process creates its
        // activation event. Retry briefly so that a rapid double-click still restores the UI.
        for (var attempt = 0; attempt < 20; attempt++)
        {
            try
            {
                using var activationEvent = EventWaitHandle.OpenExisting(activationEventName);
                activationEvent.Set();
                return;
            }
            catch (WaitHandleCannotBeOpenedException) when (attempt < 19)
            {
                Thread.Sleep(50);
            }
            catch (UnauthorizedAccessException)
            {
                return;
            }
        }
    }
}
