import Foundation
import Vision

enum ScreenCheckpoint: String {
    case browse
    case resets
    case credits
    case disallowDraft = "disallow-draft"
    case allowDraft = "allow-draft"
    case cancelled
    case returned
    case quotaAllTop = "quota-all-top"
    case quotaAllNext = "quota-all-next"
    case quotaOpenaiBottom = "quota-openai-bottom"
    case quotaClaudeBottom = "quota-claude-bottom"
    case quotaAll = "quota-all"
    case quotaChooser = "quota-chooser"
    case quotaOpenai = "quota-openai"
    case quotaClaude = "quota-claude"
    case quotaHidden = "quota-hidden"
    case quotaDraft = "quota-draft"
    case quotaCancelled = "quota-cancelled"
    case quotaRestored = "quota-restored"
    case quotaAllBottom = "quota-all-bottom"
    case quotaEmpty = "quota-empty"
    case sessionsCodexBottom = "sessions-codex-bottom"
    case sessionsClaudeBottom = "sessions-claude-bottom"
    case sessionsNewChoice = "sessions-new-choice"
    case sessionsAll = "sessions-all"
    case sessionsChooser = "sessions-chooser"
    case sessionsCodex = "sessions-codex"
    case sessionsClaude = "sessions-claude"
    case sessionsCursor = "sessions-cursor"
    case sessionsDraft = "sessions-draft"
    case sessionsCancelled = "sessions-cancelled"
    case sessionsEmpty = "sessions-empty"
    case sessionsRestored = "sessions-restored"
    case sessionsAllBottom = "sessions-all-bottom"
    case sessionsDetails = "sessions-details"
    case sessionsDetailsClosed = "sessions-details-closed"
    case quotaDetails = "quota-details"
    case quotaDetailsClosed = "quota-details-closed"

    var requiredMarkers: [String] {
        switch self {
        case .sessionsDetails:
            ["Session details", "Codex Alpha", "Session ID", "Conversation", "Close"]
        case .sessionsDetailsClosed:
            ["Codex", "2/6", "Codex Alpha", "Details F4"]
        case .quotaDetails:
            ["Account details", "Claude Gamma", "Selected account", "Close"]
        case .quotaDetailsClosed:
            ["Claude", "Disabled: Hidden", "1/4 accounts", "Details F4"]
        case .quotaAllTop:
            ["Quota status", "All", "4/4 accounts", "Claude Delta"]
        case .quotaAllNext:
            ["Quota status", "All", "4/4 accounts", "Claude Gamma"]
        case .quotaOpenaiBottom:
            ["Quota status", "OpenAI", "2/4 accounts", "OpenAI Beta"]
        case .quotaClaudeBottom:
            ["Quota status", "Claude", "2/4 accounts", "Claude Gamma"]
        case .quotaAll:
            ["Quota status", "All", "Disabled: Shown", "4/4 accounts"]
        case .quotaChooser:
            ["Quota status", "Account provider", "Draft: All", "Disabled accounts: Shown", "Apply", "Cancel"]
        case .quotaOpenai:
            ["Quota status", "OpenAI", "2/4 accounts", "OpenAI Alpha"]
        case .quotaClaude:
            ["Quota status", "Claude", "2/4 accounts", "Claude Delta"]
        case .quotaHidden:
            ["Quota status", "Claude", "Disabled: Hidden", "1/4 accounts", "Claude Gamma"]
        case .quotaDraft:
            ["Quota status", "Claude", "Disabled: Hidden", "1/4 accounts", "Draft: OpenAI", "Disabled accounts: Hidden", "Apply", "Cancel"]
        case .quotaCancelled:
            ["Quota status", "Claude", "Disabled: Hidden", "1/4 accounts", "Claude Gamma"]
        case .quotaRestored:
            ["Quota status", "All", "Disabled: Shown", "4/4 accounts"]
        case .quotaAllBottom:
            ["Quota status", "All", "4/4 accounts", "OpenAI Beta"]
        case .quotaEmpty:
            ["Quota status", "Claude", "Disabled: Hidden", "0/4 accounts", "No accounts match these filters"]
        case .sessionsCodexBottom:
            ["Sessions", "Codex", "2/6", "Codex Beta"]
        case .sessionsClaudeBottom:
            ["Sessions", "Claude Code", "2/6", "Claude Delta"]
        case .sessionsNewChoice:
            ["Sessions", "Cursor", "1/6", "New Codex session"]
        case .sessionsAll:
            ["Sessions", "All providers", "6/6"]
        case .sessionsChooser:
            ["Session client provider", "Draft: All providers", "Apply", "Cancel", "Claude Code", "Cursor"]
        case .sessionsCodex:
            ["Sessions", "Codex", "2/6", "Codex Alpha"]
        case .sessionsClaude:
            ["Sessions", "Claude Code", "2/6", "Claude Gamma"]
        case .sessionsCursor:
            ["Sessions", "Cursor", "1/6", "Cursor Echo"]
        case .sessionsDraft:
            ["Session client provider", "Draft: Codex", "Cursor", "1/6", "Apply", "Cancel"]
        case .sessionsCancelled:
            ["Sessions", "Cursor", "1/6", "Cursor Echo"]
        case .sessionsEmpty:
            ["Sessions", "Cursor", "0/6", "New Codex session"]
        case .sessionsRestored:
            ["Sessions", "All providers", "6/6"]
        case .sessionsAllBottom:
            ["Sessions", "All providers", "6/6", "Claude-lookalike Zeta"]
        case .browse, .returned:
            ["Quota status", "pty-alpha", "account options"]
        case .resets:
            ["Quota status", "Reset credits", "Weekly remaining"]
        case .credits, .cancelled:
            ["Quota status", "Credit balance", "42.00", "Disallow", "enter edit"]
        case .allowDraft:
            ["Quota status", "Credit usage", "42.00", "› Allow", "enter save"]
        case .disallowDraft:
            ["Quota status", "Credit usage", "42.00", "› Disallow", "enter save"]
        }
    }

    var forbiddenMarkers: [String] {
        switch self {
        case .sessionsDetails, .quotaDetails:
            ["Draft:", "Apply", "Cancel"]
        case .sessionsDetailsClosed, .quotaDetailsClosed:
            ["Session details", "Account details", "Draft:"]
        case .quotaAllTop, .quotaAllNext:
            ["Account provider"]
        case .quotaOpenaiBottom:
            ["Claude Gamma", "Claude Delta", "Account provider"]
        case .quotaClaudeBottom:
            ["OpenAI Alpha", "OpenAI Beta", "Account provider"]
        case .quotaAll:
            ["Account provider"]
        case .quotaChooser:
            []
        case .quotaOpenai:
            ["Claude Gamma", "Claude Delta", "Account provider"]
        case .quotaClaude:
            ["OpenAI Alpha", "OpenAI Beta", "Account provider"]
        case .quotaHidden:
            ["Claude Delta", "OpenAI Alpha", "OpenAI Beta", "Account provider"]
        case .quotaDraft:
            ["Claude Delta"]
        case .quotaCancelled:
            ["Account provider", "Claude Delta", "OpenAI Alpha", "OpenAI Beta"]
        case .quotaRestored:
            ["Account provider"]
        case .quotaAllBottom:
            ["Account provider"]
        case .quotaEmpty:
            ["Claude Gamma", "Claude Delta", "OpenAI Alpha", "OpenAI Beta", "Account provider"]
        case .sessionsCodexBottom:
            ["Claude Gamma", "Claude Delta", "Cursor Echo", "Claude-lookalike Zeta", "Session client provider"]
        case .sessionsClaudeBottom:
            ["Codex Alpha", "Codex Beta", "Cursor Echo", "Claude-lookalike Zeta", "Session client provider"]
        case .sessionsNewChoice:
            ["Session client provider", "Codex Alpha", "Codex Beta", "Claude Gamma", "Claude Delta"]
        case .sessionsAll:
            ["Session client provider"]
        case .sessionsChooser:
            []
        case .sessionsCodex:
            ["Claude Gamma", "Claude Delta", "Cursor Echo", "Claude-lookalike Zeta", "Session client provider"]
        case .sessionsClaude:
            ["Codex Alpha", "Codex Beta", "Cursor Echo", "Claude-lookalike Zeta", "Session client provider"]
        case .sessionsCursor:
            ["Codex Alpha", "Codex Beta", "Claude Gamma", "Claude Delta", "Claude-lookalike Zeta", "Session client provider"]
        case .sessionsDraft:
            ["Codex Alpha", "Codex Beta", "Claude Gamma", "Claude Delta"]
        case .sessionsCancelled:
            ["Session client provider", "Codex Alpha", "Codex Beta", "Claude Gamma", "Claude Delta"]
        case .sessionsEmpty:
            ["Cursor Echo", "Codex Alpha", "Codex Beta", "Claude Gamma", "Claude Delta", "Session client provider"]
        case .sessionsRestored:
            ["Session client provider"]
        case .sessionsAllBottom:
            ["Session client provider"]
        case .browse, .returned:
            ["Credit balance", "Reset credits"]
        case .resets:
            ["Credit balance"]
        case .credits, .cancelled, .allowDraft, .disallowDraft:
            []
        }
    }
}

struct ScreenImageObservation: Codable {
    let checkpoint: String
    let recognizedLines: [String]
    let missingMarkers: [String]
    let unexpectedMarkers: [String]
}

enum ImageVerificationError: Error {
    case invalidArguments
}

@main
struct VerifyScreenImage {
    static func main() throws {
        let arguments = CommandLine.arguments
        guard arguments.count == 3,
              let checkpoint = ScreenCheckpoint(rawValue: arguments[1]) else {
            throw ImageVerificationError.invalidArguments
        }
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.usesLanguageCorrection = false
        request.recognitionLanguages = ["en-US"]
        let handler = VNImageRequestHandler(url: URL(fileURLWithPath: arguments[2]), options: [:])
        try handler.perform([request])
        let recognizedLines = (request.results ?? []).compactMap { $0.topCandidates(1).first?.string }
        let recognizedText = recognizedLines.joined(separator: "\n")
            .replacingOccurrences(of: ">", with: "›")
            // Vision sometimes joins the chevron and label at the narrow width.
            // Preserve the selection marker and full Allow token; never accept
            // the pane's unrelated explanatory text as a selected draft.
            .replacingOccurrences(of: "›[ \t]*Allow\\b", with: "› Allow", options: .regularExpression)
            .replacingOccurrences(of: "›[ \t]*Disallow\\b", with: "› Disallow", options: .regularExpression)
            // OCR can split the count and unit into adjacent observations even
            // when they occupy one painted toolbar row. Normalize only this
            // literal count/unit pair; keep draft and provider markers intact.
            .replacingOccurrences(of: "([0-9]+/[0-9]+)[ \t\r\n]+(accounts|sessions)\\b", with: "$1 $2", options: .regularExpression)
            .lowercased()
        let missingMarkers = checkpoint.requiredMarkers.filter { !recognizedText.contains($0.lowercased()) }
        let unexpectedMarkers = checkpoint.forbiddenMarkers.filter { recognizedText.contains($0.lowercased()) }
        let observation = ScreenImageObservation(
            checkpoint: checkpoint.rawValue,
            recognizedLines: recognizedLines,
            missingMarkers: missingMarkers,
            unexpectedMarkers: unexpectedMarkers
        )
        FileHandle.standardOutput.write(try JSONEncoder().encode(observation))
        FileHandle.standardOutput.write(Data("\n".utf8))
        if !missingMarkers.isEmpty || !unexpectedMarkers.isEmpty {
            exit(1)
        }
    }
}
