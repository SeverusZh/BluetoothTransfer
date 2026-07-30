using System.Windows;

namespace BluetoothTransfer.Services;

public class ThemeService
{
    private static readonly Uri LightThemeUri = new("pack://application:,,,/Themes/LightTheme.xaml");
    private static readonly Uri DarkThemeUri = new("pack://application:,,,/Themes/DarkTheme.xaml");

    private ResourceDictionary? _currentTheme;

    public string CurrentTheme { get; private set; } = "light";

    public void ApplyTheme(string theme)
    {
        var uri = theme == "dark" ? DarkThemeUri : LightThemeUri;
        var newDict = new ResourceDictionary { Source = uri };

        var appResources = Application.Current.Resources;
        if (_currentTheme != null)
            appResources.MergedDictionaries.Remove(_currentTheme);

        appResources.MergedDictionaries.Add(newDict);
        _currentTheme = newDict;
        CurrentTheme = theme;
    }

    public void ToggleTheme()
    {
        ApplyTheme(CurrentTheme == "dark" ? "light" : "dark");
    }
}
