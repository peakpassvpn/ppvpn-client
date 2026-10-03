using Microsoft.UI.Xaml.Controls;
using PPVPN.App.Core.ViewModels;
using PPVPN.Windows.Strings;

namespace PPVPN.Windows.Views;

public sealed partial class LoginView : UserControl
{
    public LoginView()
    {
        InitializeComponent();
    }

    public MainViewModel ViewModel => App.ViewModel;

    /// <summary>"在浏览器中登录", or after an error "重试" / "重新登录".</summary>
    public static string ButtonText(bool hasError, string retryText) =>
        hasError && retryText.Length > 0 ? retryText : Loc.Get("loginBtn");
}
