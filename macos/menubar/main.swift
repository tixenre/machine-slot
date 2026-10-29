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

    var running: Bool { kind == "solo" || kind == "shared" }
    var elapsed: Int { start.map { max(0, Int(Date().timeIntervalSince1970) - $0) } ?? 0 }
}

func readState() -> [Job] {
    run(["state"]).split(separator: "\n").map { line in
        let f = line.split(separator: "\t", omittingEmptySubsequences: false).map(String.init)
        func at(_ i: Int) -> String { i < f.count ? f[i] : "" }
        return Job(kind: at(0), label: at(1), start: Int(at(2)), usual: Int(at(3)), progress: Double(at(5)), measured: at(6) == "measured",
                   cpu: Double(at(7)), memory: Int(at(8)), disk: Double(at(9)), startsIn: Int(at(10)), quiet: Int(at(11)), settlingSince: Int(at(12)), settlingOn: at(13))
    }
}

func clock(_ s: Int) -> String { s >= 60 ? "\(s / 60):" + String(format: "%02d", s % 60) : "\(s)s" }

/// One job: name, a bar against its usual time, what it's doing.
final class Row: NSView {
    let title = NSTextField(labelWithString: "")
    let detail = NSTextField(labelWithString: "")
    let bar = NSProgressIndicator()
    let usage = NSTextField(labelWithString: "")

    init() {
        super.init(frame: NSRect(x: 0, y: 0, width: 380, height: 50))
        title.font = .systemFont(ofSize: 13, weight: .medium)
        title.lineBreakMode = .byTruncatingTail
        detail.font = .monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        detail.textColor = .secondaryLabelColor
        detail.lineBreakMode = .byTruncatingTail
        usage.font = .monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        usage.textColor = .tertiaryLabelColor
        usage.lineBreakMode = .byTruncatingTail
        bar.style = .bar
        bar.controlSize = .small
        bar.minValue = 0
        bar.maxValue = 1
        for v in [title, detail, bar, usage] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        NSLayoutConstraint.activate([
            title.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 18),
            title.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
            title.topAnchor.constraint(equalTo: topAnchor, constant: 4),
            bar.leadingAnchor.constraint(equalTo: title.leadingAnchor),
            bar.trailingAnchor.constraint(equalTo: title.trailingAnchor),
            bar.topAnchor.constraint(equalTo: title.bottomAnchor, constant: 3),
            detail.leadingAnchor.constraint(equalTo: title.leadingAnchor),
            detail.trailingAnchor.constraint(equalTo: title.trailingAnchor),
            detail.topAnchor.constraint(equalTo: bar.bottomAnchor, constant: 2),
            usage.leadingAnchor.constraint(equalTo: title.leadingAnchor),
            usage.trailingAnchor.constraint(equalTo: title.trailingAnchor),
            usage.topAnchor.constraint(equalTo: detail.bottomAnchor, constant: 1),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    func show(_ j: Job, place: Int?) {
        let alone = j.kind == "solo" || j.kind == "draining" || j.kind == "queued"
        title.stringValue = (alone ? "🔒 " : "") + j.label
        if let c = j.cpu, let m = j.memory {
            let mem = m >= 1024 ? String(format: "%.1f GB", Double(m) / 1024) : "\(m) MB"
            var u = "CPU \(Int(c.rounded())) % · \(mem)"
            if let d = j.disk, d >= 0.5 { u += " · disk \(Int(d.rounded())) MB/s" }
            if let q = j.quiet, q >= 300 { u += " · nothing for \(clock(q)) (stalled?)" }
            usage.stringValue = u
            usage.isHidden = false
        } else {
            usage.isHidden = true
        }
        setFrameSize(NSSize(width: frame.width, height: usage.isHidden ? 50 : 64))
        if j.running, let since = j.settlingSince {
            bar.isHidden = false
            bar.isIndeterminate = true
            bar.startAnimation(nil)
            let waited = max(0, Int(Date().timeIntervalSince1970) - since)
            detail.stringValue = "Waiting \(clock(waited)) for a quiet machine"
            usage.stringValue = j.settlingOn
            usage.isHidden = j.settlingOn.isEmpty
            setFrameSize(NSSize(width: frame.width, height: usage.isHidden ? 50 : 64))
            return
        }
        if j.running {
            bar.isHidden = false
            if let p = j.progress {
                bar.stopAnimation(nil)
                bar.isIndeterminate = false
                bar.doubleValue = p
                let pct = "\(j.measured ? "" : "~")\(Int((p * 100).rounded())) %"
                if j.measured, p > 0.02, j.elapsed > 2 {
                    let rest = Int((Double(j.elapsed) / p - Double(j.elapsed)).rounded())
                    detail.stringValue = "\(pct) — \(clock(j.elapsed)) so far, ~\(clock(max(0, rest))) left"
                } else if let u = j.usual {
                    detail.stringValue = j.elapsed <= u
                        ? "\(pct) — \(clock(j.elapsed)) of ~\(clock(u)), \(clock(u - j.elapsed)) left"
                        : "\(pct) — \(clock(j.elapsed)), longer than usual (~\(clock(u)))"
                } else {
                    detail.stringValue = "\(pct) — \(clock(j.elapsed))"
                }
            } else {
                bar.isIndeterminate = true
                bar.startAnimation(nil)
                detail.stringValue = "\(clock(j.elapsed)) — no history yet"
            }
        } else {
            bar.isHidden = true
            var why: String
            switch j.kind {
            case "draining": why = "Next: runs alone once the running work ends"
            case "queued": why = "Queued to run alone" + (place.map { " (#\($0))" } ?? "")
            default: why = "Waits for the job running alone"
            }
            if let s = j.startsIn { why += " · starts in ~\(clock(s))" }
            if let u = j.usual { why += " · usually ~\(clock(u))" }
            detail.stringValue = why
        }
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
