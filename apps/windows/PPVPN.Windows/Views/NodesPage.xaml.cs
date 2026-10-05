using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using PPVPN.App.Core.ViewModels;

namespace PPVPN.Windows.Views;

/// <summary>
/// Flat node table (design §3): click selects (<see cref="NodesViewModel.SelectedItem"/>),
/// double-click sets the current node, right-click opens the context menu; the bottom card shows
/// the selected node's local proxy.
/// </summary>
public sealed partial class NodesPage : Page
{
    public NodesPage()
    {
        InitializeComponent();
        var segments = new[] { MethodIcmp, MethodTcp, MethodConnect };
        segments[Math.Clamp(ViewModel.MethodIndex, 0, 2)].IsChecked = true;

        ActualThemeChanged += (_, _) =>
        {
            // Row tone colours are resolved in code; rebuild rows for the new theme.
            var selected = ViewModel.SelectedItem;
            Table.ItemsSource = null;
            Table.ItemsSource = ViewModel.Items;
            ViewModel.SelectedItem = selected;
            Bindings.Update();
        };
    }

    /// <summary>
    /// A row's line picker: only real choices go to the view model (one-way binding here, so a
    /// ComboBox that clears its selection when Lines is re-read never writes null back).
    /// </summary>
    void OnLineChanged(object sender, SelectionChangedEventArgs e)
    {
        if (sender is FrameworkElement { DataContext: NodeItemViewModel item }
            && e.AddedItems.FirstOrDefault() is LineOption line
            && line != item.SelectedLine)
            item.SelectedLine = line;
    }

    public NodesViewModel ViewModel => App.ViewModel.Nodes;

    public static Visibility IsLatency(LatencyKind kind, string name) => Ui.Visible(kind.ToString() == name);

    public static Visibility IsView(NodesViewState state, string name) => Ui.Visible(state.ToString() == name);

    public static Visibility NotView(NodesViewState state, string name) => Ui.Visible(state.ToString() != name);

    public static Visibility ShowProxyBar(NodesViewState state, bool hasSelection) => Ui.Visible(state == NodesViewState.Data && hasSelection);

    /// <summary>"HTTP 127.0.0.1:7890"; the protocol alone while there is no proxy.</summary>
    public static string ProxyLabel(string protocol, ProxyInfo? proxy) => proxy is null ? protocol : $"{protocol} {proxy.Endpoint}";

    /// <summary>" · 正在测速（TCP）" after the node count, or nothing.</summary>
    public static string Dot(string text) => string.IsNullOrEmpty(text) ? "" : " · " + text;

    void OnMethodChecked(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { Tag: string tag } && int.TryParse(tag, out var index) && index != ViewModel.MethodIndex)
            ViewModel.MethodIndex = index;
    }

    /// <summary>Double-click sets the current node.</summary>
    void OnRowDoubleTapped(object sender, DoubleTappedRoutedEventArgs e)
    {
        var row = (e.OriginalSource as FrameworkElement)?.DataContext as NodeItemViewModel ?? ViewModel.SelectedItem;
        if (row?.SetCurrentCommand.CanExecute(null) == true) row.SetCurrentCommand.Execute(null);
    }
}
