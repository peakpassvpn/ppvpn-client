using System.Collections.ObjectModel;
using System.Collections.Specialized;
using System.ComponentModel;

namespace PPVPN.App.Core.ViewModels;

/// <summary>
/// An <see cref="ObservableCollection{T}"/> whose contents can be replaced with a single
/// <see cref="NotifyCollectionChangedAction.Reset"/>, so a list view re-renders once instead of
/// once per item (WinUI and GTK both handle a reset; multi-item adds are not used).
/// </summary>
public sealed class BulkObservableCollection<T> : ObservableCollection<T>
{
    public void ReplaceAll(IEnumerable<T> items)
    {
        CheckReentrancy();
        Items.Clear();
        foreach (var item in items) Items.Add(item);
        OnPropertyChanged(new PropertyChangedEventArgs(nameof(Count)));
        OnPropertyChanged(new PropertyChangedEventArgs("Item[]"));
        OnCollectionChanged(new NotifyCollectionChangedEventArgs(NotifyCollectionChangedAction.Reset));
    }
}
