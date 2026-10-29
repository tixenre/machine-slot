# Homebrew formula, for a tap repository (tixenre/homebrew-tap):
#   brew install tixenre/tap/machine-slot
# On each release, set `url` to the release's source tarball and `sha256`
# to its checksum (`shasum -a 256`).
class MachineSlot < Formula
  desc "Machine-wide queue for heavy work: builds share, benchmarks run alone"
  homepage "https://github.com/tixenre/machine-slot"
  url "https://github.com/tixenre/machine-slot/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "REPLACE_WITH_THE_TARBALL_SHA256"
  license any_of: ["MIT", "Apache-2.0"]
  head "https://github.com/tixenre/machine-slot.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
    pkgshare.install "integrations"
    if OS.mac?
      cd "macos/menubar" do
        system "./build.sh", "--no-open"
        prefix.install "build/Machine Slot.app"
      end
    end
  end

  def caveats
    s = <<~EOS
      Claude Code integration (hook, skill, MCP server):
        #{opt_pkgshare}/integrations/claude-code/install.sh
    EOS
    if OS.mac?
      s += <<~EOS

        Menu bar app:
          cp -R "#{opt_prefix}/Machine Slot.app" ~/Applications/
      EOS
    end
    s
  end

  test do
    assert_match "free", shell_output("#{bin}/machine-slot status")
    assert_match "machine-slot #{version}", shell_output("#{bin}/machine-slot --version")
  end
end
