class Codocia < Formula
  desc "Documentation drift checker for AI coding agents"
  homepage "https://github.com/lhwzds/codocia"
  version "0.1.2"
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

  test do
    assert_match "Codocia Docs Skill", shell_output("#{bin}/codocia skill")
  end
end
