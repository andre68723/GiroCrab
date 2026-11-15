import SwiftUI
import Charts
import GiroCrabCore

/// The value line of one or more securities depots. A value holds until the next known one (a step
/// line, never a smoothed curve: nothing is suggested between two valuations), a partial sum is
/// dashed, and the line breaks where another depot joins. Amounts use the depots' currency.
/// Hover (or a touch) names the day, its value and the change since the start of the period.
struct DepotValueChart: View {
    let points: [DepotValuePoint]
    var currency = "EUR"
    /// The shown period: the axis spans it even where the first value comes later (nil: the data).
    var period: ClosedRange<Date>?
    /// The ISO day under the pointer.
    @State private var hoveredDay: String?

    var body: some View {
        let segments = DepotValueHistory.segments(DepotValueHistory.drawn(points))
        let domain = yDomain
        Chart {
            ForEach(Array(segments.enumerated()), id: \.offset) { index, segment in
                ForEach(segment.points, id: \.date) { point in
                    if let day = DateText.parseISODay(point.date) {
                        let euros = Double(point.valueCents) / 100
                        if !segment.isPartial {
                            // a soft fill under the solid stretches; a partial sum is only a dashed line
                            AreaMark(x: .value("Datum", day), yStart: .value("Basis", domain.lowerBound),
                                     yEnd: .value("Wert", euros), series: .value("Fläche", index))
                                .interpolationMethod(.stepEnd)
                                .foregroundStyle(LinearGradient(colors: [Theme.accent.opacity(0.14), Theme.accent.opacity(0.01)],
                                                                startPoint: .top, endPoint: .bottom))
                        }
                        LineMark(x: .value("Datum", day), y: .value("Wert", euros), series: .value("Abschnitt", index))
                            .interpolationMethod(.stepEnd)
                            .lineStyle(StrokeStyle(lineWidth: 2, dash: segment.isPartial ? [4, 3] : []))
                            .foregroundStyle(Theme.accent)
                    }
                }
            }
            if let hovered, let hoveredDay, let day = DateText.parseISODay(hoveredDay) {
                RuleMark(x: .value("Datum", day))
                    .foregroundStyle(Theme.tertiary)
                    .lineStyle(StrokeStyle(lineWidth: 1))
                PointMark(x: .value("Datum", day), y: .value("Wert", Double(hovered.valueCents) / 100))
                    .symbolSize(40)
                    .foregroundStyle(Theme.accent)
            }
        }
        .chartYScale(domain: domain)
        .chartXScale(domain: xDomain)
        .chartYAxis {
            AxisMarks(position: .trailing, values: .automatic(desiredCount: 3)) { value in
                AxisGridLine()
                AxisValueLabel {
                    if let euros = value.as(Double.self) {
                        Text(Money.format(Int64((euros * 100).rounded()), currency: currency))
                            .appFont(.caption2, monospacedDigit: true).fixedSize()
                    }
                }
            }
        }
        .chartXAxis {
            // fixedSize: without it the last label is cut to "…"
            AxisMarks(values: .automatic(desiredCount: 3)) { value in
                AxisValueLabel {
                    if let date = value.as(Date.self) {
                        Text(date, format: longSpan ? .dateTime.month(.abbreviated).year() : .dateTime.day().month(.abbreviated))
                            .appFont(.caption2).fixedSize()
                    }
                }
            }
        }
        .chartOverlay { proxy in
            GeometryReader { geo in
                Color.clear
                    .contentShape(Rectangle())
                    #if os(macOS)
                    .onContinuousHover { phase in
                        switch phase {
                        case .active(let location): hoveredDay = day(at: location, proxy: proxy, geo: geo)
                        case .ended: hoveredDay = nil
                        }
                    }
                    #else
                    .gesture(DragGesture(minimumDistance: 0)
                        .onChanged { hoveredDay = day(at: $0.location, proxy: proxy, geo: geo) }
                        .onEnded { _ in hoveredDay = nil })
                    #endif
            }
        }
        .overlay(alignment: .topLeading) { readout }
        .accessibilityLabel(summary)
    }

    /// The point whose value holds on the hovered day.
    private var hovered: DepotValuePoint? {
        hoveredDay.flatMap { DepotValueHistory.value(on: $0, in: points) }
    }

    private func day(at location: CGPoint, proxy: ChartProxy, geo: GeometryProxy) -> String? {
        guard let plot = proxy.plotFrame else { return nil }
        let x = location.x - geo[plot].origin.x
        guard let date: Date = proxy.value(atX: x) else { return nil }
        return DateText.iso(date)
    }

    /// "Mo. 9. Nov. 2026 · 54.170,00 € · +1.200,00 € (+2,3 %)": the change only against the same coverage.
    @ViewBuilder
    private var readout: some View {
        if let hovered, let first = points.first {
            let change = DepotValueHistory.change(of: hovered, since: first)
            HStack(spacing: 6) {
                Text(DateText.medium(hoveredDay ?? hovered.date)).foregroundStyle(Theme.secondary)
                Text(Money.format(hovered.valueCents, currency: currency)).fontWeight(.semibold)
                if let change {
                    Text("\(Money.signed(change.cents, currency: currency))\(change.percent.map { " (\(DepotFormat.percent($0, signed: true, digits: 1)))" } ?? "")")
                        .foregroundStyle(DepotFormat.tint(change.cents))
                }
            }
            .appFont(.caption2, monospacedDigit: true)
            .padding(.horizontal, 6).padding(.vertical, 2)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 5))
            .fixedSize()
            .allowsHitTesting(false)
        }
    }

    /// Close to the values (a depot's value moves by a few percent: from zero or a wide automatic
    /// domain the line would look flat), with a little air above and below.
    private var yDomain: ClosedRange<Double> {
        let values = points.map { Double($0.valueCents) / 100 }
        guard let lo = values.min(), let hi = values.max() else { return 0...1 }
        let pad = max((hi - lo) * 0.12, abs(hi) * 0.004, 1)
        return (lo - pad)...(hi + pad)
    }

    /// More than about a year on the axis: the labels name the year (two "1. Jan." would be ambiguous).
    private var longSpan: Bool {
        let domain = xDomain
        return domain.upperBound.timeIntervalSince(domain.lowerBound) > 400 * 86_400
    }

    private var xDomain: ClosedRange<Date> {
        let first = points.first.flatMap { DateText.parseISODay($0.date) } ?? Date()
        let last = points.last.flatMap { DateText.parseISODay($0.date) } ?? first
        let lo = min(period?.lowerBound ?? first, first), hi = max(period?.upperBound ?? last, last)
        return lo...max(hi, lo.addingTimeInterval(86_400))
    }

    private var summary: String {
        guard let first = points.first, let last = points.last else { return "" }
        return String(localized: "Depotwert von \(Money.format(first.valueCents, currency: currency)) am \(first.date) auf \(Money.format(last.valueCents, currency: currency)) am \(last.date)")
    }
}
