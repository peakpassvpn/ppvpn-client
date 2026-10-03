using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using PPVPN.App.Core.ViewModels;

namespace PPVPN.Windows.Controls;

/// <summary>
/// ComboBox over <see cref="TeamOption"/>s that disables the items of inactive teams
/// (<see cref="TeamOption.IsSelectable"/> false). They stay listed, greyed out, like the
/// other apps; the view model also ignores a selection of one.
/// </summary>
public sealed class TeamComboBox : ComboBox
{
    public TeamComboBox()
    {
        DefaultStyleKey = typeof(ComboBox);
    }

    protected override void PrepareContainerForItemOverride(DependencyObject element, object item)
    {
        base.PrepareContainerForItemOverride(element, item);
        if (element is ComboBoxItem container && item is TeamOption team) container.IsEnabled = team.IsSelectable;
    }
}
