import PPVPNAppLogic
import PPVPNClient
import SwiftUI

struct ProbeLabel: View {
    let outcome: ProbeOutcome?

    var body: some View {
        switch outcome {
        case nil:
            Text("—").foregroundStyle(.tertiary)
        case .running:
            ProgressView().controlSize(.small)
        case .latency(let ms):
            Text("\(ms) ms")
                .monospacedDigit()
                .foregroundStyle(ms < 100 ? .green : ms < 250 ? .orange : .red)
        case .failed(.timeout):
            Text("超时").foregroundStyle(.red)
        case .failed(let code):
            Text("失败").foregroundStyle(.red).help(code.message)
        }
    }
}
