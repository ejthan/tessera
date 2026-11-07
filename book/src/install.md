# Install

Tessera runs on macOS 13 or later, on Windows 10 and 11, and on Linux x86_64 with Wayland or X11.

The installers of a release are built by CI, on each OS, and each release lists them with a `SHA256SUMS` file. Check a download against that file before you open it. The first release's installers come from the first run of that build, so a format below may still have rough edges. Tell us in an issue if one does.

To run Tessera without a package, build it from source, as `docs/building.md` in the repository describes.

## macOS

Download the `.dmg` for your Mac: `aarch64` for Apple silicon, `x64` for Intel. Open it and drag Tessera to Applications.

Tessera is signed ad-hoc and is not notarized, so macOS warns at the first open. After you have checked the download, clear the quarantine flag:

```bash
xattr -cr /Applications/Tessera.app
```

**After an update,** macOS may ask again for access to the keys Tessera stored in your keychain, because the new build has a new signature. Choose Always Allow for it to stop asking until the next update.

There is no Homebrew tap yet.

## Windows

- **`.msi`:** installs for all users and adds Start menu and desktop shortcuts. A newer `.msi` upgrades an older one. The installer shows no license dialog; the license files are installed with the app.
- **`.zip`:** unpack it anywhere and start `tessera.exe`. The license files are beside it.

Tessera opens no console window on Windows, so `tessera --help` prints nothing there. The flags are in [Command-line flags](cli.md).

## Linux

- **`.deb`,** for Ubuntu 24.04 and similar:

  ```bash
  sudo apt install ./tessera_<version>_amd64.deb
  ```

- **`.rpm`,** for Fedora and similar:

  ```bash
  sudo dnf install ./tessera-<version>*.rpm
  ```

- **AppImage:** make it executable and start it. If your system has no FUSE, add `--appimage-extract-and-run`.

  ```bash
  chmod +x tessera_<version>_x86_64.AppImage
  ./tessera_<version>_x86_64.AppImage
  ```

Tessera needs a working Vulkan driver, and a Secret Service to store keys. See [Troubleshooting](troubleshooting.md) if either is missing.
