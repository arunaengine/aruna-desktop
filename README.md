# Aruna Desktop

Aruna Desktop connects your computer to an Aruna realm. It runs a small Aruna node on your
machine and shows the same portal you use on the web. Your files sync with the realm, and your
jobs can run on your computer.

## How to install

### Before you start

- **A realm account.** You need an account on an Aruna realm, for example
  [v3.aruna-engine.org](https://v3.aruna-engine.org).
- **The matching version.** The app version must match the version the realm runs. Download the
  release whose number equals the realm's version (the realm shows it on its portal). Later
  versions install from inside the app.
- **Local jobs (optional).** To run jobs on this computer, install
  [Docker](https://docs.docker.com/get-docker/). On Linux, [Apptainer](https://apptainer.org/)
  works too. Without either, the app still syncs files.

### Download

Open the [releases page](https://github.com/arunaengine/aruna-desktop/releases) and pick the
release that matches your realm:

| System | File ending in | Notes |
| --- | --- | --- |
| Linux (Debian, Ubuntu) | `.deb` | x86_64 |
| Linux (Fedora, RHEL, openSUSE) | `.rpm` | x86_64 |
| Linux (any) | `.AppImage` | x86_64, no installation needed |
| Windows 10 and 11 | `-setup.exe` | x86_64 |
| macOS | `.dmg` | Apple silicon (M1 or newer) only |

### Linux

Open a terminal in your downloads folder and run the command for your system. If that folder holds
older Aruna files, type the full file name instead of `Aruna*`.

```bash
sudo apt install ./Aruna*.deb        # Debian, Ubuntu
sudo dnf install ./Aruna*.rpm        # Fedora, RHEL (openSUSE: sudo zypper install ./Aruna*.rpm)
chmod +x ./Aruna*.AppImage && ./Aruna*.AppImage   # AppImage, no installation
```

Some distributions need `libfuse2` for AppImages (`sudo apt install libfuse2` on Ubuntu).

### Windows

Run the downloaded `-setup.exe` file. The installer is not signed yet, so Windows SmartScreen may
show "Windows protected your PC". Click **More info**, then **Run anyway**.

### macOS

Open the `.dmg` and drag **Aruna Desktop** into **Applications**. The app is not notarized by
Apple yet, so macOS warns on the first start:

- **macOS 14 and older:** right-click **Aruna Desktop** in **Applications**, choose **Open**, and
  confirm with **Open**.
- **macOS 15 and newer:** try to start the app once, then open **System Settings**, go to
  **Privacy & Security**, and choose **Open Anyway** next to the message about Aruna Desktop.

If macOS still refuses, run this once in Terminal:

```bash
xattr -dr com.apple.quarantine "/Applications/Aruna Desktop.app"
```

### First start

1. **Connect to your realm.** Enter the realm address, for example `v3.aruna-engine.org`.
2. **Sign in** with your realm account. The sign-in page opens in a separate window and closes by
   itself when you finish.
3. **Add this computer as a device.** Open **Settings**, then **Access & connection**. Under
   **Devices**, choose **Add a device**, and on the last step **Open in Aruna Desktop**.
4. **Wait for setup to finish.** When it is done, your computer is listed as one of your devices.

The setup code expires. If setup fails because the code is "expired, already used, or not
valid", create a new code with **Add a device**.

### Updates

When your realm moves to a newer version, the app shows an update screen and installs the
matching version in the same way you first installed it. On macOS, restart the app afterward.

### Uninstall

Remove the app the usual way for your system. Your local data stays in the app's data folder
until you delete it: `~/.local/share/engine.aruna.desktop` on Linux,
`%APPDATA%\engine.aruna.desktop` on Windows, and
`~/Library/Application Support/engine.aruna.desktop` on macOS. Files already synced to the realm
stay on the realm.

## What you can do

- **Use your realm.** Browse, upload and share data in the same portal as on the web.
- **Link a folder.** In the data view, choose **Link a local folder** to keep a local folder and a
  realm bucket in sync, one way or both ways.
- **Run jobs locally.** Jobs started on **This computer** run here with Docker or Apptainer. You
  set how many run at once and how much CPU, memory and disk they may use in the device settings.
- **Pause.** A paused device runs nothing: no sync and no local jobs. Resume to start it again.
- **Quit.** Closing the window quits the app and stops the local node cleanly.

## License

Dual licensed under [Apache 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT).
