import SwiftUI
import UIKit

/// Bubble-gum look: soft pink-white cards, plum rounded text in ordinary case,
/// big round corners, pill buttons and one candy-pink accent. Light and dark.
/// Plum means working, pink means selected or needing attention, mauve is idle.
enum Theme {
    /// Colours follow the phone's appearance: paper is the card, ink is the text on it.
    private static func shade(_ light: (CGFloat, CGFloat, CGFloat), _ dark: (CGFloat, CGFloat, CGFloat)) -> UIColor {
        UIColor {
            let c = $0.userInterfaceStyle == .dark ? dark : light
            return UIColor(red: c.0, green: c.1, blue: c.2, alpha: 1)
        }
    }
    static let uiPaper = shade((1.00, 0.97, 0.99), (0.17, 0.11, 0.20))
    static let uiWell = shade((0.95, 0.92, 1.00), (0.23, 0.16, 0.29))
    static let uiInk = shade((0.28, 0.15, 0.32), (1.00, 0.94, 0.97))
    static let uiDim = shade((0.56, 0.44, 0.60), (0.77, 0.66, 0.80))
    static let uiRule = shade((0.96, 0.82, 0.90), (0.36, 0.25, 0.41))
    static let uiAccent = UIColor(red: 1.00, green: 0.44, blue: 0.71, alpha: 1)

    static let paper = Color(uiColor: uiPaper)
    /// Viewport backgrounds: the mini map, the POV view, the joystick.
    static let well = Color(uiColor: uiWell)
    static let ink = Color(uiColor: uiInk)
    static let dim = Color(uiColor: uiDim)
    static let rule = Color(uiColor: uiRule)
    static let accent = Color(uiColor: uiAccent)
    /// Text and marks drawn on the accent, the same in both appearances.
    static let onAccent = Color(red: 0.29, green: 0.07, blue: 0.22)
    /// The soft glow under cards and floating controls.
    static let glow = Color(red: 0.80, green: 0.30, blue: 0.60).opacity(0.22)

    /// Corner radius of cards; viewports and fields use a little less.
    static let corner: CGFloat = 26

    static func font(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight, design: .rounded)
    }
    static func uiFont(_ size: CGFloat, _ weight: UIFont.Weight = .regular) -> UIFont {
        let base = UIFont.systemFont(ofSize: size, weight: weight)
        return base.fontDescriptor.withDesign(.rounded).map { UIFont(descriptor: $0, size: size) } ?? base
    }
}

extension View {
    /// A short label: section titles, tags, button text.
    func themeLabel(_ size: CGFloat = 11, _ weight: Font.Weight = .regular) -> some View {
        font(Theme.font(size + 1.5, weight == .regular ? .medium : weight))
    }
    /// Running text: status sentences and captions.
    func themeText(_ size: CGFloat = 11, _ weight: Font.Weight = .regular) -> some View {
        font(Theme.font(size + 1, weight))
    }
    /// A soft card with round corners and a pink glow.
    func themePanel(padding: CGFloat = 18) -> some View {
        self.padding(padding)
            .foregroundStyle(Theme.ink)
            .tint(Theme.accent)
            .background(Theme.paper, in: RoundedRectangle(cornerRadius: Theme.corner, style: .continuous))
            .shadow(color: Theme.glow, radius: 16, y: 6)
    }
    /// A small pill floating over a viewport.
    func themeTag() -> some View {
        self.padding(.horizontal, 11).padding(.vertical, 6)
            .foregroundStyle(Theme.ink)
            .background(Theme.paper.opacity(0.94), in: Capsule())
            .shadow(color: Theme.glow, radius: 5, y: 2)
    }
    /// A small card floating over a viewport, for more than one line.
    func themeBubble() -> some View {
        self.padding(10)
            .background(Theme.paper.opacity(0.94), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .shadow(color: Theme.glow, radius: 5, y: 2)
    }
    /// A round icon button floating over a viewport.
    func themeDot() -> some View {
        frame(width: 34, height: 34)
            .background(Theme.paper.opacity(0.94), in: Circle())
            .shadow(color: Theme.glow, radius: 4, y: 2)
            .contentShape(Circle())
    }
    /// A viewport: lilac well with round corners.
    func themeWell() -> some View {
        background(Theme.well).clipShape(RoundedRectangle(cornerRadius: Theme.corner - 6, style: .continuous))
    }
    /// A list row that shows selection as a pink bubble.
    func themeRow(selected: Bool) -> some View {
        self.padding(.vertical, 8).padding(.horizontal, 10)
            .background(selected ? Theme.accent.opacity(0.22) : Color.clear,
                        in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .contentShape(Rectangle())
    }
    /// System forms in the same paper, rounded type and accent.
    func themeForm() -> some View {
        fontDesign(.rounded)
            .tint(Theme.accent)
            .scrollContentBackground(.hidden)
            .background(Theme.paper)
    }
}

/// A row of beads that separates groups in a card.
struct Dots: View {
    var body: some View {
        Canvas { context, size in
            var x: CGFloat = 2
            while x < size.width {
                context.fill(Path(ellipseIn: CGRect(x: x, y: size.height / 2 - 1.5, width: 3, height: 3)), with: .color(Theme.rule))
                x += 9
            }
        }
        .frame(height: 7)
        .accessibilityHidden(true)
    }
}

/// Four rounded corners: the frame drawn around a target.
struct Brackets: Shape {
    var arm: CGFloat = 10
    func path(in r: CGRect) -> Path {
        var path = Path()
        for (x, y, dx, dy) in [(r.minX, r.minY, 1.0, 1.0), (r.maxX, r.minY, -1.0, 1.0),
                               (r.maxX, r.maxY, -1.0, -1.0), (r.minX, r.maxY, 1.0, -1.0)] {
            path.move(to: CGPoint(x: x + dx * arm, y: y))
            path.addArc(tangent1End: CGPoint(x: x, y: y), tangent2End: CGPoint(x: x, y: y + dy * arm), radius: arm * 0.8)
            path.addLine(to: CGPoint(x: x, y: y + dy * arm))
        }
        return path
    }
}

/// A circle that is filled when on: the status light and the checkbox.
struct CheckBox: View {
    let on: Bool
    var color = Theme.accent
    var size: CGFloat = 20
    var body: some View {
        ZStack {
            Circle().fill(on ? color : Theme.well)
            Circle().stroke(on ? color : Theme.rule, lineWidth: 1.5)
            if on && size >= 12 {
                Image(systemName: "checkmark").font(.system(size: size * 0.55, weight: .heavy)).foregroundStyle(Theme.onAccent)
            }
        }
        .frame(width: size, height: size)
    }
}

/// A toggle drawn as a round checkbox with its label beside it.
struct CheckToggleStyle: ToggleStyle {
    @Environment(\.isEnabled) private var enabled
    func makeBody(configuration: Configuration) -> some View {
        Button { withAnimation(.spring(response: 0.25, dampingFraction: 0.6)) { configuration.isOn.toggle() } } label: {
            HStack(spacing: 10) {
                CheckBox(on: configuration.isOn)
                configuration.label.themeLabel(12).foregroundStyle(Theme.ink)
                Spacer(minLength: 0)
            }
            .padding(.vertical, 4).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .opacity(enabled ? 1 : 0.4)
        .accessibilityAddTraits(configuration.isOn ? .isSelected : [])
    }
}

/// A pill button that squishes when pressed. `filled` is the one primary action on a card.
struct PillButtonStyle: ButtonStyle {
    var filled = false
    func makeBody(configuration: Configuration) -> some View { Pill(configuration: configuration, filled: filled) }

    private struct Pill: View {
        let configuration: Configuration
        let filled: Bool
        @Environment(\.isEnabled) private var enabled
        var body: some View {
            let destructive = configuration.role == .destructive
            configuration.label
                .themeLabel(12, .bold)
                .lineLimit(1)
                .padding(.horizontal, 16).padding(.vertical, 10)
                .foregroundStyle(filled ? Theme.onAccent : Theme.ink)
                .background(filled ? Theme.accent : destructive ? Theme.accent.opacity(0.28) : Theme.well, in: Capsule())
                .shadow(color: filled ? Theme.glow : .clear, radius: 6, y: 3)
                .opacity(enabled ? 1 : 0.4)
                .scaleEffect(configuration.isPressed ? 0.93 : 1)
                .animation(.spring(response: 0.25, dampingFraction: 0.55), value: configuration.isPressed)
                .contentShape(Capsule())
        }
    }
}

/// Squishes whatever label it is given, for buttons that draw their own shape.
struct SquishButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.93 : 1)
            .animation(.spring(response: 0.25, dampingFraction: 0.55), value: configuration.isPressed)
    }
}

/// A text field in a soft rounded well.
struct SoftFieldStyle: TextFieldStyle {
    func _body(configuration: TextField<Self._Label>) -> some View {
        configuration
            .font(Theme.font(14))
            .foregroundStyle(Theme.ink)
            .padding(.horizontal, 12).padding(.vertical, 8)
            .background(Theme.well, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }
}

/// Tabs as pills in a lilac track, the selected one filled pink.
struct TabStrip: View {
    let tabs: [(tag: String, title: String)]
    @Binding var selection: String
    var body: some View {
        HStack(spacing: 0) {
            ForEach(tabs, id: \.tag) { tab in
                let selected = selection == tab.tag
                Button { withAnimation(.spring(response: 0.3, dampingFraction: 0.7)) { selection = tab.tag } } label: {
                    Text(tab.title).themeLabel(12, selected ? .bold : .medium)
                        .foregroundStyle(selected ? Theme.onAccent : Theme.dim)
                        .padding(.vertical, 8).frame(maxWidth: .infinity)
                        .background(selected ? Theme.accent : Color.clear, in: Capsule())
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(selected ? .isSelected : [])
            }
        }
        .padding(4)
        .background(Theme.well, in: Capsule())
    }
}
