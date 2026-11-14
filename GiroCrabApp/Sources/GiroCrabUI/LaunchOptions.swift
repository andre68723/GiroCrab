import Foundation

/// Command line / environment switches.
///
///  --demo-seed            fill a throw-away vault with demo data for all 7 banks
///  GIROCRAB_DEMO_SEED=1   same, via environment (works for Xcode schemes / iOS)
///  --demo-sync            (with demo data) start "Sync All" right away, to
///                         see the push-TAN / code / redirect 2FA sheets
///  --view alltag|sparplaene|belege  start in "Alltag & Konsum" / "Sparpläne & Perks" / "Belege & PostBox"
///  --fresh-vault          (developer) empty personal vault in a temp folder,
///                         fixed key, no Keychain – to try onboarding/setup
///  --keep-fresh-vault     (developer) like --fresh-vault, but reopen the temp vault
///                         as it is instead of wiping it (e.g. a restored copy)
///  --developer            (developer) show developer tools in the UI, e.g.
///                         "Portal-Struktur prüfen"; also GIROCRAB_DEVELOPER=1
///  --try-add-bank <name>  (developer, with --fresh-vault) add a bank on start,
///                         e.g. 1822direkt, to see the setup form
///  --select <text>        (demo) preselect the account whose name contains <text>
///  --demo-sheet account-selection|transfer|moneymoney|moneymoney-db|recurring|print|rules|palette|
///               categorizer|payee-match|payee-close|payee-nomatch|error|rule-edit|icon-color|category|icon-color-category
///                         (demo) open that sheet pre-filled with fictional data,
///                         for README / PR screenshots
///  --demo-settings general|banks|groups|documents|security[:edit|:add|:remove|:select]
///                         (demo) open the Settings window on that tab (`groups` opens the
///                         group editor sheet instead), optionally with
///                         the first bank's edit sheet or the "add bank" sheet;
///                         --screenshot then captures the Settings window
///  --demo-filter income|expense
///                         (demo) start with the booking list filtered to
///                         income or expenses (PR screenshots, #53)
///  --dark                 (macOS) force the Dark Aqua appearance (README screenshots)
///  --light                (macOS, `swift run`) force the light Aqua appearance
///  --simulate-system-language <code>
///                         (developer) pretend the system language is <code> (e.g. en);
///                         the app's language choice is kept in the separate defaults
///                         domain de.girocrab.app.language-simulation (#93)
///  --show-tips           (developer) reset TipKit and show tips in the demo
///                         vault too (screenshots of contextual tips)
///  --demo-menu <title>    (macOS, with --screenshot and GIROCRAB_SCREENSHOT_NATIVE=1)
///                         pop up that menu-bar menu over the window and capture it
///  --screenshot <file>    (macOS) render the main window, save a PNG, quit.
///                         Implies --demo-seed so the picture is reproducible.
public struct LaunchOptions: Sendable {
    public var demoSeed: Bool
    public var screenshotPath: String?
    public var dark: Bool
    public var demoSync: Bool
    /// "alle" (default), "alltag" or "sparplaene"
    public var initialView: String
    public var freshVault: Bool
    /// (developer) reopen the --fresh-vault file without wiping it first
    public var keepFreshVault: Bool
    /// (developer) show developer-only buttons
    public var developer: Bool
    public var tryAddBank: String?
    /// (demo) preselect the account whose name contains this text
    public var selectAccount: String?
    /// (demo) preselect the booking whose merchant/counterparty contains this text
    public var pickBooking: String?
    /// (demo) "transfer", "moneymoney", "moneymoney-db" or "print": open that sheet with fictional data
    public var demoSheet: String?
    /// (demo) "income" or "expense": start with that booking list filter
    public var demoFilter: String?
    /// (demo) Settings tab to open on start, e.g. "banks" or "banks:edit"
    public var demoSettings: String?
    /// (demo) title of the menu-bar menu to show in the screenshot
    public var demoMenu: String?
    /// (developer) pretended system language, see `LanguagePreference.current`
    public var simulateSystemLanguage: String?
    /// (developer) reset TipKit and show tips in the demo vault too
    public var showTips: Bool

    public init(arguments: [String] = CommandLine.arguments, environment: [String: String] = ProcessInfo.processInfo.environment) {
        var demo = arguments.contains("--demo-seed")
        if let env = environment["GIROCRAB_DEMO_SEED"], ["1", "true", "yes"].contains(env.lowercased()) {
            demo = true
        }
        var shot: String?
        if let i = arguments.firstIndex(of: "--screenshot"), i + 1 < arguments.count {
            shot = arguments[i + 1]
        }
        if let i = arguments.firstIndex(of: "--select"), i + 1 < arguments.count {
            selectAccount = arguments[i + 1].lowercased()
        } else {
            selectAccount = nil
        }
        if let i = arguments.firstIndex(of: "--pick"), i + 1 < arguments.count {
            pickBooking = arguments[i + 1].lowercased()
        } else {
            pickBooking = nil
        }
        if let i = arguments.firstIndex(of: "--demo-sheet"), i + 1 < arguments.count {
            demoSheet = arguments[i + 1].lowercased()
        } else {
            demoSheet = nil
        }
        if let i = arguments.firstIndex(of: "--demo-filter"), i + 1 < arguments.count {
            demoFilter = arguments[i + 1].lowercased()
        } else {
            demoFilter = nil
        }
        if let i = arguments.firstIndex(of: "--demo-settings"), i + 1 < arguments.count {
            demoSettings = arguments[i + 1].lowercased()
        } else {
            demoSettings = nil
        }
        if let i = arguments.firstIndex(of: "--demo-menu"), i + 1 < arguments.count {
            demoMenu = arguments[i + 1]
        } else {
            demoMenu = nil
        }
        let sync = arguments.contains("--demo-sync")
        keepFreshVault = arguments.contains("--keep-fresh-vault")
        freshVault = arguments.contains("--fresh-vault") || keepFreshVault
        developer = arguments.contains("--developer") || environment["GIROCRAB_DEVELOPER"] == "1"
        if let i = arguments.firstIndex(of: "--try-add-bank"), i + 1 < arguments.count {
            tryAddBank = arguments[i + 1].lowercased()
        } else {
            tryAddBank = nil
        }
        if (shot != nil || sync || demoSheet != nil || demoSettings != nil) && !freshVault { demo = true }
        self.demoSync = sync
        if let i = arguments.firstIndex(of: "--view"), i + 1 < arguments.count {
            initialView = arguments[i + 1].lowercased()
        } else {
            initialView = "alle"
        }
        self.demoSeed = demo
        self.screenshotPath = shot
        self.dark = arguments.contains("--dark")
        if let i = arguments.firstIndex(of: "--simulate-system-language"), i + 1 < arguments.count {
            simulateSystemLanguage = arguments[i + 1]
        } else {
            simulateSystemLanguage = nil
        }
        self.showTips = arguments.contains("--show-tips")
    }

    public static let current = LaunchOptions()
}
