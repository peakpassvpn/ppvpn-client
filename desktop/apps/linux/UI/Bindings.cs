using System.Collections.Specialized;
using System.ComponentModel;

namespace PPVPN.Linux.UI;

/// <summary>
/// One-way bindings from the shared view models to GTK widgets. View models raise their events
/// on the GTK main thread (SynchronizedListener), so handlers touch widgets directly.
/// </summary>
public static class Bindings
{
    /// <summary>Runs <paramref name="apply"/> now and whenever one of <paramref name="properties"/> changes.</summary>
    public static IDisposable Bind(this INotifyPropertyChanged source, Action apply, params string[] properties)
    {
        void Handler(object? sender, PropertyChangedEventArgs args)
        {
            if (string.IsNullOrEmpty(args.PropertyName) || properties.Contains(args.PropertyName)) apply();
        }

        source.PropertyChanged += Handler;
        apply();
        return new Subscription(() => source.PropertyChanged -= Handler);
    }

    /// <summary>
    /// Runs <paramref name="rebuild"/> now and after changes to the collection. View models
    /// refill collections with Clear() and one Add() per item, so the changes of one main-loop
    /// iteration are coalesced into a single rebuild.
    /// </summary>
    public static IDisposable BindItems(this INotifyCollectionChanged source, Action rebuild)
    {
        var context = SynchronizationContext.Current
            ?? throw new InvalidOperationException("bind collections on the GTK main thread");
        var pending = false;
        var active = true;
        void Handler(object? sender, NotifyCollectionChangedEventArgs args)
        {
            if (pending) return;
            pending = true;
            context.Post(_ =>
            {
                pending = false;
                if (active) rebuild();
            }, null);
        }

        source.CollectionChanged += Handler;
        rebuild();
        return new Subscription(() =>
        {
            active = false;
            source.CollectionChanged -= Handler;
        });
    }

    /// <summary>Shows the widget only while <paramref name="visible"/> holds.</summary>
    public static IDisposable BindVisible(this INotifyPropertyChanged source, Gtk.Widget widget, Func<bool> visible, params string[] properties) =>
        source.Bind(() => widget.SetVisible(visible()), properties);

    public sealed class Subscription(Action dispose) : IDisposable
    {
        private Action? _dispose = dispose;

        public void Dispose() => Interlocked.Exchange(ref _dispose, null)?.Invoke();
    }
}

/// <summary>Subscriptions owned by rows that get rebuilt.</summary>
public sealed class SubscriptionBag : IDisposable
{
    private readonly List<IDisposable> _items = [];

    public void Add(IDisposable item) => _items.Add(item);

    public void Dispose()
    {
        foreach (var item in _items) item.Dispose();
        _items.Clear();
    }
}
