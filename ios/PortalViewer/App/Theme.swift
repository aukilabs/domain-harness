import SwiftUI
import UIKit

/// Instrument-panel look: flat paper panels, ink monospace labels in capitals,
/// hairline rules, square corners, and a single orange accent. Light and dark.
/// Ink means working, orange means selected or needing attention, grey is idle.
enum Theme {
    /// Greys follow the phone's appearance: paper is the panel, ink is the text on it.
    private static func grey(_ light: CGFloat, _ dark: CGFloat) -> UIColor {
        UIColor { $0.userInterfaceStyle == .dark ? UIColor(white: dark, alpha: 1) : UIColor(white: light, alpha: 1) }
    }
    static let uiPaper = grey(0.965, 0.09)
    static let uiWell = grey(0.84, 0.17)
    static let uiInk = grey(0.12, 0.93)
    static let uiDim = grey(0.44, 0.60)
    static let uiRule = grey(0.72, 0.30)
    static let uiAccent = UIColor(red: 0.93, green: 0.39, blue: 0.19, alpha: 1)

    static let paper = Color(uiColor: uiPaper)
    /// Viewport backgrounds: the mini map, the POV view, the joystick.
    static let well = Color(uiColor: uiWell)
    static let ink = Color(uiColor: uiInk)
    static let dim = Color(uiColor: uiDim)
    static let rule = Color(uiColor: uiRule)
    static let accent = Color(uiColor: uiAccent)
    /// Text and marks drawn on the accent, the same in both appearances.
    static let onAccent = Color(white: 0.1)

    static func mono(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight, design: .monospaced)
    }
    static func uiMono(_ size: CGFloat, _ weight: UIFont.Weight = .regular) -> UIFont {
        .monospacedSystemFont(ofSize: size, weight: weight)
    }
}

extension View {
    /// A short label in capitals: section titles, tags, button text.
    func themeLabel(_ size: CGFloat = 11, _ weight: Font.Weight = .regular) -> some View {
        font(Theme.mono(size, weight)).tracking(0.8).textCase(.uppercase)
    }
    /// Running text: status sentences and captions.
    func themeText(_ size: CGFloat = 11, _ weight: Font.Weight = .regular) -> some View {
        font(Theme.mono(size, weight))
    }
    /// A flat paper panel with a hairline edge.
    func themePanel(padding: CGFloat = 14) -> some View {
        self.padding(padding)
            .foregroundStyle(Theme.ink)
            .tint(Theme.accent)
            .background(Theme.paper)
            .overlay(Rectangle().stroke(Theme.rule, lineWidth: 0.5))
    }
    /// A small paper tag floating over a viewport.
    func themeTag() -> some View {
        self.padding(.horizontal, 7).padding(.vertical, 5)
            .foregroundStyle(Theme.ink)
            .background(Theme.paper.opacity(0.94))
            .overlay(Rectangle().stroke(Theme.rule, lineWidth: 0.5))
    }
    /// A viewport: grey well, hairline edge, square corners.
    func themeWell() -> some View {
        background(Theme.well).clipShape(Rectangle())
            .overlay(Rectangle().stroke(Theme.rule, lineWidth: 0.5))
    }
    /// A list row that shows selection with a tint and an orange bar on its leading edge.
    func themeRow(selected: Bool) -> some View {
        self.padding(.vertical, 7).padding(.horizontal, 8)
            .background(selected ? Theme.accent.opacity(0.16) : Color.clear)
            .overlay(alignment: .leading) {
                Rectangle().fill(selected ? Theme.accent : Color.clear).frame(width: 2)
            }
            .contentShape(Rectangle())
    }
    /// System forms in the same paper, monospace and accent.
    func themeForm() -> some View {
        fontDesign(.monospaced)
            .tint(Theme.accent)
            .scrollContentBackground(.hidden)
            .background(Theme.paper)
    }
}

/// A band of diagonal hatching that separates groups in a panel.
struct Hatch: View {
    var body: some View {
        Canvas { context, size in
            var path = Path()
            var x = -size.height
            while x < size.width {
                path.move(to: CGPoint(x: x, y: size.height))
                path.addLine(to: CGPoint(x: x + size.height, y: 0))
                x += 4
            }
            context.stroke(path, with: .color(Theme.rule), lineWidth: 0.6)
        }
        .frame(height: 7)
        .accessibilityHidden(true)
    }
}

/// Four corner brackets: the reticle drawn around a target.
struct Brackets: Shape {
    var arm: CGFloat = 10
    func path(in r: CGRect) -> Path {
        var path = Path()
        for (x, y, dx, dy) in [(r.minX, r.minY, 1.0, 1.0), (r.maxX, r.minY, -1.0, 1.0),
                               (r.maxX, r.maxY, -1.0, -1.0), (r.minX, r.maxY, 1.0, -1.0)] {
            path.move(to: CGPoint(x: x + dx * arm, y: y))
            path.addLine(to: CGPoint(x: x, y: y))
            path.addLine(to: CGPoint(x: x, y: y + dy * arm))
        }
        return path
    }
}

/// A square that is filled when on: the status light and the checkbox.
struct CheckBox: View {
    let on: Bool
    var color = Theme.accent
    var size: CGFloat = 14
    var body: some View {
        ZStack {
            Rectangle().fill(on ? color : Color.clear)
            Rectangle().stroke(on ? color : Theme.dim, lineWidth: 1)
            if on && size >= 12 {
                Image(systemName: "checkmark").font(.system(size: size * 0.6, weight: .bold)).foregroundStyle(Theme.onAccent)
            }
        }
        .frame(width: size, height: size)
    }
}

/// A toggle drawn as a checkbox with its label in capitals.
struct CheckToggleStyle: ToggleStyle {
    @Environment(\.isEnabled) private var enabled
    func makeBody(configuration: Configuration) -> some View {
        Button { configuration.isOn.toggle() } label: {
            HStack(spacing: 10) {
                CheckBox(on: configuration.isOn)
                configuration.label.themeLabel().foregroundStyle(Theme.ink)
                Spacer(minLength: 0)
            }
            .padding(.vertical, 4).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .opacity(enabled ? 1 : 0.4)
        .accessibilityAddTraits(configuration.isOn ? .isSelected : [])
    }
}

/// A flat rectangular button. `filled` is the one primary action on a panel.
struct PlateButtonStyle: ButtonStyle {
    var filled = false
    func makeBody(configuration: Configuration) -> some View { Plate(configuration: configuration, filled: filled) }

    private struct Plate: View {
        let configuration: Configuration
        let filled: Bool
        @Environment(\.isEnabled) private var enabled
        var body: some View {
            let destructive = configuration.role == .destructive
            configuration.label
                .themeLabel(11, .semibold)
                .lineLimit(1)
                .padding(.horizontal, 12).padding(.vertical, 8)
                .foregroundStyle(filled ? Theme.onAccent : destructive ? Theme.accent : Theme.ink)
                .background(filled ? Theme.accent : Color.clear)
                .overlay(Rectangle().stroke(filled || destructive ? Theme.accent : Theme.ink, lineWidth: 1))
                .opacity(!enabled ? 0.35 : configuration.isPressed ? 0.6 : 1)
                .contentShape(Rectangle())
        }
    }
}

/// A text field on a hairline rule.
struct RuleFieldStyle: TextFieldStyle {
    func _body(configuration: TextField<Self._Label>) -> some View {
        configuration
            .font(Theme.mono(13))
            .foregroundStyle(Theme.ink)
            .padding(.vertical, 5)
            .overlay(alignment: .bottom) { Rectangle().fill(Theme.rule).frame(height: 1) }
    }
}

/// Tabs as capitals on a rule, the selected one underlined in orange.
struct TabStrip: View {
    let tabs: [(tag: String, title: String)]
    @Binding var selection: String
    var body: some View {
        HStack(spacing: 0) {
            ForEach(tabs, id: \.tag) { tab in
                let selected = selection == tab.tag
                Button { selection = tab.tag } label: {
                    VStack(spacing: 6) {
                        Text(tab.title).themeLabel(11, selected ? .semibold : .regular)
                            .foregroundStyle(selected ? Theme.ink : Theme.dim)
                        ZStack(alignment: .bottom) {
                            Rectangle().fill(Theme.rule).frame(height: 0.5)
                            Rectangle().fill(selected ? Theme.accent : Color.clear).frame(height: 2)
                        }
                    }
                    .padding(.top, 6).frame(maxWidth: .infinity).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(selected ? .isSelected : [])
            }
        }
    }
}
