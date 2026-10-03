import PPVPNAppLogic
import PPVPNClient
import SwiftUI

enum SettingsTab: String {
    static let storageKey = "settingsTab"
    case general, account, advanced
}

enum AdvancedSettingsKeys {
    static let apiBase = "apiBaseOverride"
}

/// Settings window (⌘,) with icon tabs: General · Account · Advanced.
struct SettingsView: View {
    @AppStorage(SettingsTab.storageKey) private var tab = SettingsTab.general

    var body: some View {
        TabView(selection: $tab) {
            GeneralSettings()
                .tabItem { Label(tr("general"), systemImage: "gearshape") }
                .tag(SettingsTab.general)
            AccountSettings()
                .tabItem { Label(tr("account"), systemImage: "person.crop.circle") }
                .tag(SettingsTab.account)
            AdvancedSettings()
                .tabItem { Label(tr("advanced"), systemImage: "wrench.and.screwdriver") }
                .tag(SettingsTab.advanced)
        }
        .frame(width: 500)
    }
}

// MARK: - General

private struct GeneralSettings: View {
    @AppStorage(Appearance.storageKey) private var appearance = Appearance.system
    @StateObject private var loginItem = LoginItem()
    @ObservedObject private var updater = Updater.shared
    @State private var loginItemError: String?

    var body: some View {
        Form {
            Section {
                Picker(tr("appearance"), selection: $appearance) {
                    ForEach(Appearance.allCases) { Text($0.title).tag($0) }
                }
            }
            Section {
                Toggle(tr("launchAtLogin"), isOn: Binding(get: { loginItem.isEnabled }, set: { enabled in
                    do { try loginItem.set(enabled) } catch { loginItemError = error.localizedDescription }
                }))
                if loginItem.needsApproval {
                    HStack {
                        Text(tr("launchApprove")).font(.callout).foregroundStyle(.secondary)
                        Spacer()
                        Button(tr("openSysSettings")) { loginItem.openSystemSettings() }
                    }
                }
                if let loginItemError {
                    Text(loginItemError).font(.caption).foregroundStyle(Brand.danger)
                }
                Toggle(tr("autoUpdate"), isOn: Binding(
                    get: { updater.isConfigured && updater.automaticallyChecks },
                    set: { updater.automaticallyChecks = $0 }))
                    .disabled(!updater.isConfigured)
            }
            HStack {
                Text(tr("version", ["v": Bundle.main.shortVersion, "b": Bundle.main.buildVersion]))
                    .foregroundStyle(.secondary)
                Spacer()
                CheckForUpdatesButton()
            }
        }
        .formStyle(.grouped)
        .onAppear { loginItem.refresh() }
    }
}

// MARK: - Account

private struct AccountSettings: View {
    @EnvironmentObject private var model: AppModel
    @State private var confirmingSignOut = false

    var body: some View {
        Form {
            if model.isSignedIn {
                Section {
                    LabeledContent(tr("user"), value: model.snapshot.account.map { $0.email ?? $0.name } ?? "—")
                    Picker(tr("team"), selection: teamBinding) {
                        ForEach(model.teams) { team in
                            Text(team.active ? teamName(team) : "\(teamName(team))  \(tr("disabled"))")
                                .tag(Optional(team.id))
                                .disabled(!team.active)
                        }
                    }
                    LabeledContent(tr("expires")) {
                        if let restriction = model.restriction, restriction.code != .teamDisabled {
                            Text(restriction.title)
                        } else if let expiresAt = model.profileExpiresAt {
                            Text(expiresAt, format: .dateTime.year().month(.abbreviated).day())
                        } else {
                            Text("—")
                        }
                    }
                }
                HStack {
                    Spacer()
                    Button(tr("signOut") + "…") { confirmingSignOut = true }
                }
            } else {
                Text(tr("notSignedIn")).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .confirmationDialog(tr("signOutQ"), isPresented: $confirmingSignOut) {
            Button(tr("signOut"), role: .destructive) { model.signOut() }
            Button(tr("cancel"), role: .cancel) {}
        } message: {
            Text(tr("signOutD"))
        }
    }

    private func teamName(_ team: Team) -> String { team.personal ? tr("personal") : team.name }

    private var teamBinding: Binding<String?> {
        Binding(get: { model.snapshot.team?.id }, set: { id in
            if let team = model.teams.first(where: { $0.id == id }) { model.switchTeam(team) }
        })
    }
}

// MARK: - Advanced

private struct AdvancedSettings: View {
    @EnvironmentObject private var model: AppModel
    @AppStorage(AdvancedSettingsKeys.apiBase) private var apiBase = ""
    @State private var explainingInstall = false
    @State private var confirmingUninstall = false

    var body: some View {
        Form {
            Section {
                Picker(tr("connMethod"), selection: Binding(
                    get: { model.connectionMode }, set: { model.setConnectionMode($0) })) {
                    ForEach(ConnectionMode.allCases) { mode in
                        VStack(alignment: .leading, spacing: 1) {
                            Text(mode.title)
                            Text(mode.detail).font(.caption).foregroundStyle(.secondary)
                        }
                        .padding(.vertical, 2)
                        .tag(mode)
                    }
                }
                .pickerStyle(.radioGroup)
                .labelsHidden()
            } header: {
                Text(tr("connMethod"))
            } footer: {
                Text(tr("connMethodHint")).font(.caption).foregroundStyle(.secondary)
            }
            Section {
                Picker(tr("routingMode"), selection: Binding(
                    get: { model.routingMode }, set: { model.setRoutingMode($0) })) {
                    ForEach(RoutingMode.allCases) { mode in
                        VStack(alignment: .leading, spacing: 1) {
                            Text(mode.title)
                            Text(mode.detail).font(.caption).foregroundStyle(.secondary)
                        }
                        .padding(.vertical, 2)
                        .tag(mode)
                    }
                }
                .pickerStyle(.radioGroup)
                .labelsHidden()
                Button(tr("editRoutingRules")) { model.openRoutingRulesPage() }
                    .buttonStyle(.link)
            } header: {
                Text(tr("routingMode"))
            } footer: {
                Text(tr("editRoutingRulesD")).font(.caption).foregroundStyle(.secondary)
            }
            Section {
                TextField(tr("apiOverride"), text: $apiBase, prompt: Text(tr("apiPh")))
                    .font(.system(.body, design: .monospaced))
            } footer: {
                Text(tr("apiHint")).font(.caption).foregroundStyle(.secondary)
            }
            Section {
                LabeledContent(tr("service")) {
                    HStack(spacing: 10) {
                        Text(installed ? tr("serviceOn") : tr("serviceOff")).foregroundStyle(.secondary)
                        if !installed {
                            Button(tr("installBtn")) { explainingInstall = true }
                                .disabled(!model.isSignedIn)
                        }
                    }
                }
                if installed {
                    LabeledContent(tr("uninstall")) {
                        Button(tr("uninstallBtn")) { confirmingUninstall = true }
                    }
                }
            } footer: {
                if installed {
                    Text(tr("uninstallD")).font(.caption).foregroundStyle(.secondary)
                }
            }
        }
        .formStyle(.grouped)
        .alert(tr("installT"), isPresented: $explainingInstall) {
            Button(tr("installGo")) { model.installService() }
            Button(tr("cancel"), role: .cancel) {}
        } message: {
            Text(tr("installD"))
        }
        .confirmationDialog(tr("uninstallQ"), isPresented: $confirmingUninstall) {
            Button(tr("uninstallConfirm"), role: .destructive) { model.uninstallService() }
            Button(tr("cancel"), role: .cancel) {}
        } message: {
            Text(tr("uninstallD"))
        }
    }

    private var installed: Bool { model.snapshot.serviceInstalled }
}

extension Bundle {
    var shortVersion: String { object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "—" }
    var buildVersion: String { object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "—" }
}
