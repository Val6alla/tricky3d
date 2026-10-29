# Installing tricky3D

tricky3D is free, and nobody pays for the certificates Apple and Microsoft sell to "sign" apps. Because tricky3D isn't signed, your Mac or PC warns you the first time you open it. The steps below show how to get past that warning. You only do it once per version.

Download tricky3D from the [Releases page](https://github.com/Val6alla/tricky3d/releases). Under the newest release, open **Assets** and pick the file for your computer.

| Computer | File to download |
|---|---|
| Mac with macOS 13 Ventura or later | `tricky3D_<version>_universal.dmg` |
| Windows 10 or 11 | `tricky3D_<version>_x64-setup.exe` |
| Linux | `tricky3D_<version>_amd64.AppImage`, or `tricky3D_<version>_amd64.deb` on Debian and Ubuntu |

`<version>` is the version number, for example `0.2.0`.

## Mac

The same file works on Macs with Apple chips (M1, M2 and later) and on Macs with Intel chips.

### Install

1. Download the `.dmg` file. Safari works best. Chrome sometimes blocks the download because the app isn't signed. If it does, use Safari.
2. Open your **Downloads** folder and double-click the `.dmg` file. A window opens with the tricky3D icon and an **Applications** folder.
3. Drag the tricky3D icon onto the **Applications** folder. Wait until the copy finishes.
4. Close the window. In Finder's sidebar, click the eject button next to **tricky3D**.

### Open it the first time

1. Open **Applications** in Finder and double-click **tricky3D**.
2. macOS refuses to open it and says it can't check or verify the app. Click **Done**, **OK** or **Cancel**, whichever it shows. Don't click **Move to Trash**.
3. Open the Apple menu (top left) › **System Settings** › **Privacy & Security**.
4. Scroll down to the **Security** section. Next to the line about tricky3D, click **Open Anyway**. This button only shows for about an hour after step 1. If you don't see it, do step 1 again.
5. A dialog asks again. Click **Open Anyway**, then type your Mac password or use Touch ID.

tricky3D opens. From now on it opens like any other app, from Applications, Launchpad or the Dock.

### If Open Anyway doesn't appear

Use Terminal instead. You only need to do this once.

1. Press Command and Space together, type `Terminal`, and press Return.
2. Copy this line, paste it into the Terminal window and press Return:

   ```
   xattr -dr com.apple.quarantine "/Applications/tricky3D.app"
   ```

3. Nothing is printed if it worked. Close Terminal and open tricky3D from Applications.

### Updating

Download the new `.dmg` and drag tricky3D onto **Applications** again. When asked, click **Replace**. macOS blocks the new version once more, so repeat "Open it the first time".

## Windows

### Install

1. Download the `-setup.exe` file. If your browser says the file isn't commonly downloaded, choose **Keep** (in Edge, click the **…** next to the download first).
2. Open the file from your **Downloads** folder.
3. A blue window may appear: "Windows protected your PC". Click **More info**, then **Run anyway**.
4. Follow the installer. You don't need administrator rights: tricky3D installs for your user account only.

tricky3D is now in the Start menu.

The installer contains everything tricky3D needs, including Microsoft's WebView2 component for PCs that don't have it yet. It downloads nothing.

If a window says Smart App Control blocked the installer, Windows doesn't allow any exception for one app. Smart App Control would have to be turned off in **Windows Security › App & browser control**, and on some versions of Windows 11 it can't be turned back on without reinstalling Windows. Think twice before doing that.

### Updating and uninstalling

To update, download and run the new `-setup.exe` the same way. It replaces the old version and keeps your settings.

To uninstall, open **Settings › Apps › Installed apps**, find tricky3D, click **…** and choose **Uninstall**.

## Linux

Pick one of the two files.

- **AppImage** (works on most distributions): make the file executable and run it. In a file manager, open the file's **Properties** and allow it to run as a program. In a terminal:

  ```
  chmod +x tricky3D_<version>_amd64.AppImage
  ./tricky3D_<version>_amd64.AppImage
  ```

- **.deb** (Debian, Ubuntu and their relatives): in a terminal, in the folder you downloaded it to:

  ```
  sudo apt install ./tricky3D_<version>_amd64.deb
  ```

  tricky3D then shows up with your other applications.
