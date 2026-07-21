class Codocia < Formula
  desc "One-command, source-grounded documentation generation with Codex"
  homepage "https://github.com/lhwzds/codocia"
  version "0.2.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/lhwzds/codocia/releases/download/v#{version}/codocia-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_ARM_MAC"
    else
      url "https://github.com/lhwzds/codocia/releases/download/v#{version}/codocia-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_X86_MAC"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/lhwzds/codocia/releases/download/v#{version}/codocia-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256_ARM_LINUX"
    else
      url "https://github.com/lhwzds/codocia/releases/download/v#{version}/codocia-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256_X86_LINUX"
    end
  end

  def install
    bin.install "codocia"
  end

  def caveats
    return "" unless OS.linux?

    <<~EOS
      Codex-backed `codocia generate` currently requires macOS and fails closed
      on Linux. This package still supports retained non-generation commands,
      including init, snapshot, check, site, serve, and skill.
    EOS
  end

  test do
    output = shell_output("#{bin}/codocia generate --help")
    assert_match "Generate a complete managed Markdown documentation system", output
    assert_match "--allow-code-upload", output
  end
end
