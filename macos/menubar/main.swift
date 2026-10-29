// Menu bar indicator for machine-slot: what runs, what waits, and how far
// each one is against its usual time. It reads `machine-slot state` when a
// job comes or goes (it watches the queue's folder) and every 2 s while
// something runs; with the machine free it does nothing.
import AppKit
import ServiceManagement
import UserNotifications

/// GUI apps don't get the shell's PATH: look where installs usually land.
let tool: String = {
    let home = NSHomeDirectory()
    let candidates = ["\(home)/.cargo/bin", "\(home)/.local/bin", "/opt/homebrew/bin", "/usr/local/bin"].map { "\($0)/machine-slot" }
    return candidates.first { FileManager.default.isExecutableFile(atPath: $0) } ?? "machine-slot"
}()

func run(_ args: [String]) -> String {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: tool)
    p.arguments = args
    let out = Pipe()
    p.standardOutput = out
    p.standardError = FileHandle.nullDevice
    guard (try? p.run()) != nil else { return "" }
    let data = out.fileHandleForReading.readDataToEndOfFile()
    p.waitUntilExit()
    return String(decoding: data, as: UTF8.self)
}

/// The queue's folder, as `machine-slot paths` reports it.
let stateDir: String? = {
    for line in run(["paths"]).split(separator: "\n") where line.hasPrefix("state") {
        return line.dropFirst(5).trimmingCharacters(in: .whitespaces)
    }
    return nil
}()

/// The record's folder, as `machine-slot paths` reports it.
let dataDir: String = {
    for line in run(["paths"]).split(separator: "\n") where line.hasPrefix("data") {
        return line.dropFirst(4).trimmingCharacters(in: .whitespaces)
    }
    return "\(NSHomeDirectory())/.local/share/machine-slot"
}()

struct Job {
    let kind: String  // solo, draining, queued, shared, waiting
    let label: String
    let start: Int?
    let usual: Int?
    /// 0…1: reported by the command (measured) or elapsed / usual (estimated).
    let progress: Double?
    let measured: Bool
    /// While running: CPU % of one core, memory MB, disk MB/s of its whole tree.
    let cpu: Double?
    let memory: Int?
    let disk: Double?
    /// Waiting solos: seconds until its turn, from the usual times ahead.
    let startsIn: Int?
    /// Seconds its tree has used no CPU and no disk.
    let quiet: Int?
    /// A solo waiting for a quiet machine: since when, and what isn't quiet.
    let settlingSince: Int?
    let settlingOn: String
    /// Where it was started from: «folder · branch».
    let owner: String

    var running: Bool { kind == "solo" || kind == "shared" || kind == "yielding" }
    var elapsed: Int { start.map { max(0, Int(Date().timeIntervalSince1970) - $0) } ?? 0 }
}

func readState() -> [Job] {
    run(["state"]).split(separator: "\n").map { line in
        let f = line.split(separator: "\t", omittingEmptySubsequences: false).map(String.init)
        func at(_ i: Int) -> String { i < f.count ? f[i] : "" }
        return Job(kind: at(0), label: at(1), start: Int(at(2)), usual: Int(at(3)), progress: Double(at(5)), measured: at(6) == "measured",
                   cpu: Double(at(7)), memory: Int(at(8)), disk: Double(at(9)), startsIn: Int(at(10)), quiet: Int(at(11)), settlingSince: Int(at(12)), settlingOn: at(13), owner: at(14))
    }
}

func clock(_ s: Int) -> String { s >= 60 ? "\(s / 60):" + String(format: "%02d", s % 60) : "\(s)s" }

/// A thin rounded bar: the accent color when the command reports its own
/// progress, gray when it's estimated from the usual time.
final class Meter: NSView {
    var value: Double = 0 { didSet { needsDisplay = true } }
    var measured = false { didSet { needsDisplay = true } }

    override var intrinsicContentSize: NSSize { NSSize(width: NSView.noIntrinsicMetric, height: 4) }

    override func draw(_ dirty: NSRect) {
        let r = bounds.height / 2
        NSColor.quaternaryLabelColor.setFill()
        NSBezierPath(roundedRect: bounds, xRadius: r, yRadius: r).fill()
        var done = bounds
        done.size.width = max(bounds.height, bounds.width * CGFloat(min(max(value, 0), 1)))
        (measured ? NSColor.controlAccentColor : NSColor.secondaryLabelColor).setFill()
        NSBezierPath(roundedRect: done, xRadius: r, yRadius: r).fill()
    }
}

/// One job: what it is, where it comes from, how far along, what it uses.
/// Lines that have nothing to say collapse (a stack view drops hidden views).
final class Row: NSView {
    static let width: CGFloat = 420
    let icon = NSImageView()
    let title = NSTextField(labelWithString: "")
    let owner = NSTextField(labelWithString: "")
    let meter = Meter()
    let spinner = NSProgressIndicator()
    let detail = NSTextField(labelWithString: "")
    let usage = NSTextField(labelWithString: "")
    let stack = NSStackView()

    init() {
        super.init(frame: NSRect(x: 0, y: 0, width: Row.width, height: 50))
        title.font = .systemFont(ofSize: 13, weight: .medium)
        title.lineBreakMode = .byTruncatingTail
        owner.font = .systemFont(ofSize: 11)
        owner.textColor = .tertiaryLabelColor
        owner.lineBreakMode = .byTruncatingMiddle
        detail.font = .monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        detail.textColor = .secondaryLabelColor
        detail.lineBreakMode = .byTruncatingTail
        usage.font = .monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        usage.textColor = .tertiaryLabelColor
        usage.lineBreakMode = .byTruncatingTail
        icon.symbolConfiguration = .init(pointSize: 11, weight: .semibold)
        icon.contentTintColor = .secondaryLabelColor
        spinner.style = .bar
        spinner.controlSize = .small
        spinner.isIndeterminate = true
        for f in [title, owner, detail, usage] {
            f.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        }

        let head = NSStackView(views: [icon, title])
        head.orientation = .horizontal
        head.spacing = 5
        head.alignment = .centerY
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 2
        for v in [head, owner, meter, spinner, detail, usage] as [NSView] {
            stack.addArrangedSubview(v)
        }
        stack.setCustomSpacing(4, after: owner)
        stack.setCustomSpacing(4, after: head)
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 16),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
            stack.topAnchor.constraint(equalTo: topAnchor, constant: 5),
            meter.widthAnchor.constraint(equalTo: stack.widthAnchor),
            spinner.widthAnchor.constraint(equalTo: stack.widthAnchor),
            head.widthAnchor.constraint(lessThanOrEqualTo: stack.widthAnchor),
            owner.widthAnchor.constraint(lessThanOrEqualTo: stack.widthAnchor),
            detail.widthAnchor.constraint(lessThanOrEqualTo: stack.widthAnchor),
            usage.widthAnchor.constraint(lessThanOrEqualTo: stack.widthAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    /// SF Symbol per kind: alone (locked), lending, side by side, waiting.
    static func symbol(_ kind: String) -> String {
        switch kind {
        case "solo": return "lock.fill"
        case "yielding": return "lock.open.fill"
        case "draining", "queued": return "lock"
        case "shared": return "square.stack.3d.up.fill"
        default: return "hourglass"
        }
    }

    func show(_ j: Job, place: Int?) {
        icon.image = NSImage(systemSymbolName: Row.symbol(j.kind), accessibilityDescription: j.kind)
        title.stringValue = j.label
        owner.stringValue = j.owner
        owner.isHidden = j.owner.isEmpty
        meter.isHidden = true
        spinner.isHidden = true
        usage.isHidden = true
        defer { fit() }

        if j.kind == "yielding" {
            spin(true)
            detail.stringValue = "Lending the machine to shared work, then goes on"
            return
        }
        if j.running, let since = j.settlingSince {
            spin(true)
            let waited = max(0, Int(Date().timeIntervalSince1970) - since)
            detail.stringValue = "Waiting \(clock(waited)) for a quiet machine"
            usage.stringValue = j.settlingOn
            usage.isHidden = j.settlingOn.isEmpty
            return
        }
        if j.running {
            let el = j.elapsed
            if let p = j.progress {
                spin(false)
                meter.isHidden = false
                meter.value = p
                meter.measured = j.measured
                if j.measured, p > 0.02, el > 2 {
                    let rest = Int((Double(el) / p - Double(el)).rounded())
                    detail.stringValue = "~\(clock(max(0, rest))) left · \(clock(el)) so far"
                } else if let u = j.usual {
                    detail.stringValue = el <= u
                        ? "\(clock(u - el)) left · \(clock(el)) of ~\(clock(u))"
                        : "\(clock(el)) · longer than usual (~\(clock(u)))"
                } else {
                    detail.stringValue = clock(el)
                }
            } else {
                spin(true)
                detail.stringValue = "\(clock(el)) · no history yet"
            }
            usage.stringValue = usageText(j)
            usage.isHidden = usage.stringValue.isEmpty
            return
        }
        // Waiting: when it should start and how long it takes; why it waits
        // only when that isn't just "behind the solo".
        var parts: [String] = []
        switch j.kind {
        case "draining": parts.append("Next · waits for running work to end")
        case "queued": parts.append(place.map { "#\($0) in line to run alone" } ?? "In line to run alone")
        default: break
        }
        if let s = j.startsIn { parts.append(parts.isEmpty ? "Starts in ~\(clock(s))" : "starts in ~\(clock(s))") }
        if let u = j.usual { parts.append("takes ~\(clock(u))") }
        if parts.isEmpty { parts.append("Waits for the solo to end") }
        var text = parts.joined(separator: " · ")
        text = text.prefix(1).uppercased() + text.dropFirst()
        detail.stringValue = text
    }

    /// Cores only when the tree uses some: a GPU baseline or a VM works
    /// outside it, and «0 cores» would mislead.
    func usageText(_ j: Job) -> String {
        guard let m = j.memory else { return "" }
        var parts: [String] = []
        // 100 % is one core busy; say it in cores, out of the machine's.
        if let c = j.cpu, c >= 1 {
            let cores = c / 100
            let n = ProcessInfo.processInfo.activeProcessorCount
            parts.append(cores < 9.95 ? String(format: "%.1f of %d cores", cores, n) : "\(Int(cores.rounded())) of \(n) cores")
        }
        parts.append(m >= 1024 ? String(format: "%.1f GB", Double(m) / 1024) : "\(m) MB")
        if let d = j.disk, d >= 0.5 { parts.append("disk \(Int(d.rounded())) MB/s") }
        if let q = j.quiet, q >= 300 { parts.append("nothing for \(clock(q)) (stalled?)") }
        return parts.joined(separator: " · ")
    }

    func spin(_ on: Bool) {
        spinner.isHidden = !on
        if on { spinner.startAnimation(nil) } else { spinner.stopAnimation(nil) }
    }

    /// The height its visible lines need.
    func fit() {
        stack.layoutSubtreeIfNeeded()
        setFrameSize(NSSize(width: Row.width, height: stack.fittingSize.height + 10))
    }
}

final class Bar: NSObject, NSMenuDelegate {
    let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
    var jobs: [Job] = []
    var rows: [Row] = []  // live while the menu is open
    var menuOpen = false
    var watch: DispatchSourceFileSystemObject?
    var ticks = 0
    /// Opt-in: a notification when a solo, or anything that ran a minute or
    /// more, ends.
    var notify: Bool {
        get { UserDefaults.standard.bool(forKey: "notify") }
        set { UserDefaults.standard.set(newValue, forKey: "notify") }
    }

    override init() {
        super.init()
        let menu = NSMenu()
        menu.delegate = self
        item.menu = menu
        refresh()
        watchQueue()
        let t = Timer(timeInterval: 2, repeats: true) { [weak self] _ in
            guard let self else { return }
            self.ticks += 1
            // Elapsed times and progress move only while something runs; a
            // slow look now and then covers a missed folder event.
            if !self.jobs.isEmpty || self.menuOpen || self.ticks % 30 == 0 {
                self.refresh()
            }
            if self.watch == nil, self.ticks % 15 == 0 { self.watchQueue() }
        }
        RunLoop.main.add(t, forMode: .common)  // keeps ticking while the menu is open
    }

    /// A job file comes or goes in the queue's folder: look right away.
    func watchQueue() {
        guard let dir = stateDir.map({ "\($0)/jobs" }) else { return }
        let fd = Darwin.open(dir, O_EVTONLY)
        guard fd >= 0 else { return }  // not made yet: the timer tries again
        let src = DispatchSource.makeFileSystemObjectSource(fileDescriptor: fd, eventMask: [.write, .delete, .rename], queue: .main)
        src.setEventHandler { [weak self] in
            guard let self else { return }
            if src.data.contains(.delete) || src.data.contains(.rename) {
                src.cancel()
                self.watch = nil
            }
            self.refresh()
        }
        src.setCancelHandler { Darwin.close(fd) }
        src.resume()
        watch = src
    }

    /// What ended since the last look, said once.
    func announce(ended: [Job]) {
        guard notify else { return }
        let log = run(["log", "10"])
        for j in ended where j.kind == "solo" || j.elapsed >= 60 {
            let line = log.split(separator: "\n").last { $0.hasSuffix(j.label) || $0.contains("  \(j.label)  ") }
            let failed = line.map { !$0.contains("exit 0 ") } ?? false
            let c = UNMutableNotificationContent()
            c.title = failed ? "\(j.label) failed" : "\(j.label) finished"
            c.body = "\(j.kind == "solo" ? "Ran alone" : "Ran shared") for \(clock(j.elapsed))"
                + (line?.contains("[loaded]") == true ? ", on a busy machine" : "")
            UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: UUID().uuidString, content: c, trigger: nil))
        }
    }

    /// Running first, then the waiting ones in queue order.
    func ordered() -> [Job] {
        let rank = ["solo": 0, "shared": 1, "draining": 2, "queued": 3, "waiting": 4]
        return jobs.enumerated().sorted { (rank[$0.element.kind] ?? 9, $0.offset) < (rank[$1.element.kind] ?? 9, $1.offset) }.map(\.element)
    }

    func refresh() {
        let before = jobs.filter(\.running)
        jobs = readState()
        let now = Set(jobs.filter(\.running).map { "\($0.kind)\t\($0.label)\t\($0.start ?? 0)" })
        announce(ended: before.filter { !now.contains("\($0.kind)\t\($0.label)\t\($0.start ?? 0)") })
        let solo = jobs.first { $0.kind == "solo" }
        let running = jobs.filter(\.running)
        let waiting = jobs.filter { !$0.running }
        let symbol: String
        var title = ""
        if let s = solo {
            symbol = "lock.fill"
            title = s.progress.map { "\(Int(($0 * 100).rounded())) %" } ?? clock(s.elapsed)
        } else if !running.isEmpty {
            symbol = waiting.isEmpty ? "square.stack.3d.up.fill" : "hourglass"
            title = "\(running.count)"
        } else {
            symbol = waiting.isEmpty ? "circle.dotted" : "hourglass"
        }
        if !waiting.isEmpty { title += title.isEmpty ? "+\(waiting.count)" : " +\(waiting.count)" }
        let image = NSImage(systemSymbolName: symbol, accessibilityDescription: "machine-slot")
        image?.isTemplate = true
        item.button?.image = image
        item.button?.imagePosition = .imageLeading
        item.button?.title = title.isEmpty ? "" : " " + title
        item.button?.font = .monospacedDigitSystemFont(ofSize: NSFont.systemFontSize - 1, weight: .regular)
        if menuOpen, let menu = item.menu {
            if rows.count == jobs.count { update() } else { build(menu) }
        }
    }

    func update() {
        var place = 0
        for (row, j) in zip(rows, ordered()) {
            if j.kind == "queued" { place += 1 }
            row.show(j, place: j.kind == "queued" ? place : nil)
        }
    }

    func header(_ text: String) -> NSMenuItem {
        let i = NSMenuItem(title: text, action: nil, keyEquivalent: "")
        i.attributedTitle = NSAttributedString(string: text, attributes: [
            .font: NSFont.systemFont(ofSize: 11, weight: .semibold), .foregroundColor: NSColor.secondaryLabelColor,
        ])
        return i
    }

    func build(_ menu: NSMenu) {
        menu.removeAllItems()
        rows = []
        let all = ordered()
        if all.isEmpty { menu.addItem(header("Machine free")) }
        let running = all.filter(\.running).count
        for (n, _) in all.enumerated() {
            if n == 0 && running > 0 { menu.addItem(header("Running")) }
            if n == running { menu.addItem(header("Waiting")) }
            let row = Row()
            let i = NSMenuItem()
            i.view = row
            menu.addItem(i)
            rows.append(row)
        }
        update()
        // Start a waiting job now: what it costs is said by `machine-slot go`.
        let startable = all.filter { !$0.running || $0.settlingSince != nil }
        if !startable.isEmpty {
            menu.addItem(.separator())
            let sub = NSMenu()
            for j in startable {
                let i = NSMenuItem(title: j.label, action: #selector(startNow(_:)), keyEquivalent: "")
                i.target = self
                i.representedObject = j.label
                sub.addItem(i)
            }
            let start = NSMenuItem(title: "Start Now", action: nil, keyEquivalent: "")
            start.submenu = sub
            menu.addItem(start)
        }
        menu.addItem(.separator())
        for (title, file) in [("Times per task", "times.txt"), ("Record", "runs.tsv"), ("Log", "log.txt")] {
            let i = NSMenuItem(title: title, action: #selector(open(_:)), keyEquivalent: "")
            i.target = self
            i.representedObject = "\(dataDir)/\(file)"
            menu.addItem(i)
        }
        menu.addItem(.separator())
        // Opt-in, off by default; also listed in System Settings → Login Items.
        let login = NSMenuItem(title: "Open at Login", action: #selector(toggleLogin(_:)), keyEquivalent: "")
        login.target = self
        login.state = SMAppService.mainApp.status == .enabled ? .on : .off
        menu.addItem(login)
        let note = NSMenuItem(title: "Notify When Work Ends", action: #selector(toggleNotify(_:)), keyEquivalent: "")
        note.target = self
        note.state = notify ? .on : .off
        menu.addItem(note)
        menu.addItem(NSMenuItem(title: "Quit", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"))
    }

    func menuWillOpen(_ menu: NSMenu) {
        menuOpen = true
        jobs = readState()
        build(menu)
    }

    func menuDidClose(_ menu: NSMenu) { menuOpen = false }

    /// `machine-slot go`, after saying what it costs.
    @objc func startNow(_ sender: NSMenuItem) {
        guard let label = sender.representedObject as? String else { return }
        let j = jobs.first { $0.label == label }
        let soloRuns = jobs.contains { ($0.kind == "solo" || $0.kind == "yielding") && $0.label != label }
        let alert = NSAlert()
        alert.messageText = "Start «\(label)» now?"
        if j?.kind == "waiting" && soloRuns {
            alert.informativeText = "It runs beside the solo that's measuring, so that measurement won't count as clean."
        } else if j?.settlingSince != nil {
            alert.informativeText = "It stops waiting for a quiet machine; its run is marked as loaded."
        } else if soloRuns {
            alert.informativeText = "Another solo is running; two at once would spoil both. Cancel it or wait."
        } else {
            alert.informativeText = "It skips its place in the queue."
        }
        alert.addButton(withTitle: "Start Now")
        alert.addButton(withTitle: "Cancel")
        NSApp.activate(ignoringOtherApps: true)
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let out = run(["go", label])
        if out.isEmpty {
            let fail = NSAlert()
            fail.messageText = "Couldn't start «\(label)»"
            fail.informativeText = "See `machine-slot go \"\(label)\"` in a terminal."
            fail.runModal()
        }
        refresh()
    }

    @objc func toggleNotify(_ sender: NSMenuItem) {
        if notify {
            notify = false
            return
        }
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { ok, _ in
            DispatchQueue.main.async { self.notify = ok }
        }
    }

    @objc func toggleLogin(_ sender: NSMenuItem) {
        do {
            if SMAppService.mainApp.status == .enabled {
                try SMAppService.mainApp.unregister()
            } else {
                try SMAppService.mainApp.register()
            }
        } catch {
            let alert = NSAlert()
            alert.messageText = "Couldn't change Open at Login"
            alert.informativeText = error.localizedDescription
            alert.runModal()
        }
    }

    @objc func open(_ sender: NSMenuItem) {
        guard let path = sender.representedObject as? String else { return }
        // The record and log have no app of their own: plain text.
        let editor = URL(fileURLWithPath: "/System/Applications/TextEdit.app")
        NSWorkspace.shared.open([URL(fileURLWithPath: path)], withApplicationAt: editor, configuration: NSWorkspace.OpenConfiguration())
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let bar = Bar()
app.run()
