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

    var requiredMarkers: [String] {
        switch self {
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
            .replacingOccurrences(of: "›\\s*Allow\\b", with: "› Allow", options: .regularExpression)
            .replacingOccurrences(of: "›\\s*Disallow\\b", with: "› Disallow", options: .regularExpression)
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
