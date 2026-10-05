import Combine
import PPVPNAppLogic
import SwiftUI

/// Tails the daily log files ppvpn-client writes to its log directory
/// (`ppvpn-client.YYYY-MM-DD.log`, `ppvpn-core.YYYY-MM-DD.log`, UTC dates):
/// file picker, level filter and search over the parsed entries, live
/// "following" indicator, copy of the shown entries, Finder reveal, path footer.
struct LogsView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        LogsPage(directory: model.backend.logDirectory)
    }
}

/// `LogsState` published to SwiftUI.
@MainActor
final class LogsModel: LogsState, ObservableObject {
    nonisolated(unsafe) let objectWillChange = ObservableObjectPublisher()

    override func willChange() { objectWillChange.send() }
}

private struct LogsPage: View {
    @StateObject private var logs: LogsModel

    init(directory: URL) {
        _logs = StateObject(wrappedValue: LogsModel(directory: directory))
    }

    var body: some View {
        VStack(spacing: 0) {
            actionBar
            Divider()
            LogTextView(entries: logs.shown)
                .overlay(alignment: .topLeading) {
                    if logs.shown.isEmpty {
                        Text(logs.hasEntries ? tr("logNoMatches") : tr("x_logEmpty"))
                            .foregroundStyle(.secondary)
                            .padding(14)
                    }
                }
            Divider()
            Text(footerPath)
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 14)
                .frame(height: 26)
        }
        .task {
            while !Task.isCancelled {
                logs.poll()
                try? await Task.sleep(for: .seconds(1))
            }
        }
    }

    private var actionBar: some View {
        HStack(spacing: 10) {
            Text(tr("logFile")).foregroundStyle(.secondary)
            Picker(tr("logFile"), selection: $logs.selected) {
                if logs.files.isEmpty {
                    Text("—").tag(LogFile?.none)
                }
                ForEach(logs.files) { file in
                    Text(file.title).tag(Optional(file))
                }
            }
            .labelsHidden()
            .fixedSize()
            Picker(tr("logAllLevels"), selection: $logs.level) {
                ForEach(LogLevelFilter.allCases) { Text($0.title).tag($0) }
            }
            .labelsHidden()
            .fixedSize()
            TextField(tr("logSearch"), text: $logs.search, prompt: Text(tr("logSearch")))
                .textFieldStyle(.roundedBorder)
                .frame(minWidth: 100, maxWidth: 180)
            Spacer(minLength: 8)
            if logs.selected != nil {
                FollowingIndicator()
            }
            Button {
                copyToPasteboard(logs.shownText)
            } label: {
                Label(tr("copyShownLogs"), systemImage: "doc.on.doc")
            }
            .labelStyle(.iconOnly)
            .help(tr("copyShownLogs"))
            .disabled(logs.shown.isEmpty)
            Button {
                NSWorkspace.shared.activateFileViewerSelecting([logs.selected?.url ?? logs.directory])
            } label: {
                Label(tr("revealMac"), systemImage: "folder")
            }
        }
        .controlSize(.small)
        .padding(.horizontal, 14)
        .frame(height: 40)
    }

    private var footerPath: String {
        let path = (logs.selected?.url ?? logs.directory).path
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        return path.hasPrefix(home) ? "~" + path.dropFirst(home.count) : path
    }
}

/// Green breathing dot + 「正在跟随末尾」.
private struct FollowingIndicator: View {
    @State private var dim = false

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(Brand.success)
                .frame(width: 7, height: 7)
                .opacity(dim ? 0.35 : 1)
                .animation(.easeInOut(duration: 1.4).repeatForever(autoreverses: true), value: dim)
                .onAppear { dim = true }
            Text(tr("following"))
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
        }
    }
}
