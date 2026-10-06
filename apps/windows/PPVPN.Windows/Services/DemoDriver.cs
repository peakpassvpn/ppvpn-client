using PPVPN.App.Core.Backend;
using PPVPN.App.Core.ViewModels;
using PPVPN.Ffi;

namespace PPVPN.Windows.Services;

/// <summary>
/// Executes the <c>--demo-*</c> / <c>--page</c> switches: presses the same
/// commands a user would, once the backend reaches the right state.
/// Inert without those switches (used for the VM screenshots).
/// </summary>
static class DemoDriver
{
    public static void Attach(MainViewModel vm, MainWindow window, StartupOptions options)
    {
        if (options.Page is { } page) window.NavigateTo(page);
        if (options.DemoQuitAfter is { } quitAfter) _ = QuitAsync(quitAfter);

        async Task QuitAsync(double seconds)
        {
            await Task.Delay(TimeSpan.FromSeconds(seconds));
            vm.QuitCommand.Execute(null);
        }
        if (options.DemoDialog is { } dialog && !vm.IsSignedIn && dialog == "error") _ = ShowDialogAsync(dialog);

        var signedInOnce = false;
        var catalogDone = false;
        var teamsDone = false;
        var messagesDone = false;

        void Check()
        {
            if (options.DemoSignIn && !signedInOnce && vm.Stage == AuthStage.SignedOut)
            {
                signedInOnce = true;
                vm.SignInCommand.Execute(null);
            }

            if (!catalogDone && vm.IsSignedIn && vm.Nodes.Items.Count > 0)
            {
                catalogDone = true;
                _ = RunAsync();
            }

            if (options.DemoTeams && !teamsDone && vm.HasTeamChoice)
            {
                teamsDone = true;
                _ = OpenTeamsAsync();
            }

            if (options.DemoMessages && !messagesDone && vm.IsSignedIn)
            {
                messagesDone = true;
                _ = MessagesAsync();
            }
        }

        async Task OpenTeamsAsync()
        {
            await Task.Delay(800);
            window.NavigateTo("overview");
            await Task.Delay(400);
            window.OpenAccountMenu();
        }

        async Task MessagesAsync()
        {
            await Task.Delay(1500);
            App.Current.ShowMessageCenter();
            if (!options.DemoDetail) return;
            for (var i = 0; i < 40 && vm.Inbox.Messages.Count == 0; i++) await Task.Delay(250);
            vm.Inbox.Messages.FirstOrDefault()?.ShowDetailCommand.Execute(null);
        }

        async Task RunAsync()
        {
            await Task.Delay(600);
            if (options.Page is { } target) window.NavigateTo(target);
            if (options.DemoMode is { } mode)
            {
                var wanted = mode == "compatible" ? ConnectionMode.Compatible : ConnectionMode.Enhanced;
                App.Log?.Info($"demo: connection method {wanted}");
                await vm.SetConnectionModeAsync(wanted);
            }
            if (options.DemoConnect)
            {
                App.Log?.Info("demo: connect");
                await vm.ToggleConnectCommand.ExecuteAsync(null);
            }
            if (options.DemoCopyProxy)
            {
                for (var i = 0; i < 60 && vm.RoutedProxy is null; i++) await Task.Delay(500);
                vm.RoutedProxy?.CopyHttpCommand.Execute(null);
                App.Log?.Info($"demo: proxy copied ({vm.RoutedProxy?.HttpDisplay ?? "none"})");
            }
            if (options.DemoSwitchAfter is { } switchAfter)
            {
                await Task.Delay(TimeSpan.FromSeconds(switchAfter));
                var other = vm.ConnectionMode == ConnectionMode.Compatible ? ConnectionMode.Enhanced : ConnectionMode.Compatible;
                App.Log?.Info($"demo: switch method to {other} while {vm.ConnectState}");
                await vm.SetConnectionModeAsync(other);
            }
            if (options.DemoDisconnectAfter is { } offAfter)
            {
                await Task.Delay(TimeSpan.FromSeconds(offAfter));
                App.Log?.Info($"demo: disconnect while {vm.ConnectState}");
                if (vm.ConnectState is not ConnectState.Off) await vm.ToggleConnectCommand.ExecuteAsync(null);
            }
            if (options.DemoProbe) await vm.Nodes.TestAllCommand.ExecuteAsync(null);
            if (options.DemoDialog is { } dialog) await ShowDialogAsync(dialog);
            if (options.DemoTray)
            {
                await Task.Delay(2500);
                // Bottom right, where the notification area is; the menu opens up and to the left.
                var area = Microsoft.UI.Windowing.DisplayArea.Primary.WorkArea;
                App.Current.Tray?.ShowMenuAt(area.X + area.Width - 120, area.Y + area.Height - 4);
            }
        }

        async Task ShowDialogAsync(string dialog)
        {
            await Task.Delay(1200);
            _ = dialog switch
            {
                "install" => App.Prompts.ConfirmAsync(PromptKind.InstallService),
                "uninstall" => App.Prompts.ConfirmAsync(PromptKind.UninstallService),
                "signout" => App.Prompts.ConfirmAsync(PromptKind.SignOut),
                _ => App.Prompts.ShowErrorAsync(Strings.Loc.Get("switchFailT"),
                    Strings.Loc.Format("switchFailD", ("reason", Strings.Loc.Get("Error_TeamDisabled")))),
            };
        }

        if (options.DemoConnect || options.DemoMode is not null)
            vm.PropertyChanged += (_, e) =>
            {
                if (e.PropertyName == nameof(MainViewModel.ConnectState))
                    App.Log?.Info($"demo: state {vm.ConnectionMode} {vm.ConnectState} ({vm.ConnectionTitle} · {vm.ConnectionDetail})");
            };

        vm.PropertyChanged += (_, e) =>
        {
            if (e.PropertyName is nameof(MainViewModel.Stage) or nameof(MainViewModel.Snapshot) or nameof(MainViewModel.HasTeamChoice))
                Check();
        };
        Check();
    }
}
