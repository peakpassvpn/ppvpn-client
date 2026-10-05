import PPVPNAppLogic
import PPVPNClient
import SwiftUI

/// Toolbar account button (avatar initial + team ▾): user and expiry, teams
/// (disabled ones greyed with 「已停用」), account settings, sign out.
struct AccountMenu: View {
    @EnvironmentObject private var model: AppModel
    @AppStorage(SettingsTab.storageKey) private var settingsTab = SettingsTab.general
    @State private var confirmingSignOut = false
    @Environment(\.colorScheme) private var colorScheme

    private var teamTitle: String {
        guard let team = model.snapshot.team else { return model.snapshot.account?.name ?? "" }
        return team.personal ? tr("personal") : team.name
    }

    var body: some View {
        // Wrapped so the toolbar hosts it as a view: a bare toolbar Menu is
        // turned into an icon-only NSMenuToolbarItem and drops the team name.
        HStack(spacing: 0) {
            Menu {
                AccountMenuItems(confirmingSignOut: $confirmingSignOut, settingsTab: $settingsTab)
            } label: {
                // A menu button shows one image and one title, so the
                // avatar is rendered to an image.
                Label {
                    Text(teamTitle)
                } icon: {
                    Image(nsImage: Avatar.image(name: model.snapshot.account?.name ?? "", colorScheme: colorScheme))
                }
                .labelStyle(.titleAndIcon)
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.visible)
            .fixedSize()
        }
        .padding(.horizontal, 8)
        .help(tr("account"))
        .confirmationDialog(tr("signOutQ"), isPresented: $confirmingSignOut) {
            Button(tr("signOut"), role: .destructive) { model.signOut() }
            Button(tr("cancel"), role: .cancel) {}
        } message: {
            Text(tr("signOutD"))
        }
    }
}

/// Shared by the toolbar button and the 「切换团队 ▾」 button of the
/// team-disabled empty state.
struct AccountMenuItems: View {
    @EnvironmentObject private var model: AppModel
    @Binding var confirmingSignOut: Bool
    @Binding var settingsTab: SettingsTab

    var body: some View {
        Section {
            Text(model.snapshot.account.map { $0.email ?? $0.name } ?? "")
            if let restriction = model.restriction, restriction.code != .teamDisabled {
                Text(restriction.title)
            } else if let expiresAt = model.profileExpiresAt {
                Text(tr("serviceExpiresOn", ["d": expiresAt.formatted(.dateTime.year().month(.abbreviated).day())]))
            }
        }
        Section(tr("teamsHdr")) {
            TeamItems()
        }
        Divider()
        OpenSettingsButton(beforeOpening: { settingsTab = .account }) {
            Text(tr("accountSettings"))
        }
        Button(tr("signOut") + "…") { confirmingSignOut = true }
    }
}

/// One checkmark item per team; the personal space is labelled 「个人」.
struct TeamItems: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        ForEach(model.teams) { team in
            Toggle(isOn: Binding(get: { team.id == model.snapshot.team?.id },
                                 set: { if $0 { model.switchTeam(team) } })) {
                Text(title(for: team))
            }
            .disabled(!team.active)
        }
    }

    private func title(for team: Team) -> String {
        let name = team.personal ? tr("personal") : team.name
        return team.active ? name : "\(name)  \(tr("disabled"))"
    }
}

/// Accent circle with the first letter of the account name.
struct Avatar: View {
    @MainActor static func image(name: String, colorScheme: ColorScheme, size: CGFloat = 18) -> NSImage {
        let renderer = ImageRenderer(content: Avatar(name: name, size: size).environment(\.colorScheme, colorScheme))
        renderer.scale = NSScreen.main?.backingScaleFactor ?? 2
        let image = renderer.nsImage ?? NSImage()
        image.isTemplate = false
        return image
    }

    let name: String
    var size: CGFloat = 18

    var body: some View {
        Text(name.first.map { String($0).uppercased() } ?? "?")
            .font(.system(size: size * 0.6, weight: .semibold))
            .foregroundStyle(Brand.onAccent)
            .frame(width: size, height: size)
            .background(Circle().fill(Color.accentColor))
    }
}
