cask "tessera" do
  arch arm: "aarch64", intel: "x64"

  version "1.0.0"
  sha256 arm:   "<fill from SHA256SUMS>",
         intel: "<fill from SHA256SUMS>"

  url "https://github.com/ejthan/tessera/releases/download/v#{version}/Tessera_#{version}_#{arch}.dmg"
  name "Tessera"
  desc "Native desktop client for Typesense"
  homepage "https://github.com/ejthan/tessera"

  depends_on macos: ">= :ventura"

  app "Tessera.app"

  caveats <<~EOS
    Tessera is ad-hoc signed and not notarized, so macOS warns at the first open.
    To clear the quarantine flag, run:
      xattr -cr #{appdir}/Tessera.app
  EOS
end
